use std::{
    cmp::Reverse,
    collections::HashMap,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use ahash::{AHashMap, HashMapExt, HashSetExt};
use fxhash::{FxHashMap, FxHashSet};
use rkyv::{ser::DefaultSerializer, util::AlignedVec, Archive, Deserialize, Serialize};
use roaring::RoaringBitmap;

use crate::{
    db::DB,
    pl::PostingList,
    roaringish::{Roaringish, MAX_VALUE},
    utils::{normalize, tokenize, MAX_SEQ_LEN},
    Searcher,
};

const TOKEN_ID_TOO_LONG: u32 = u32::MAX;

/// This is a macro because the compiler is dumb
/// and can't borrow check between function boundries
macro_rules! get_token_id {
    ($label:lifetime, $self:ident, $token:ident) => {
        $label: {
            if $token.as_bytes().len() > 511 {
                // break $label None;
                break $label TOKEN_ID_TOO_LONG;
            }

            let (_, token_id) = $self
                .token_to_token_id
                .raw_entry_mut()
                .from_key($token)
                .or_insert_with(|| {
                    let current_token_id = $self.next_token_id;
                    $self.next_token_id += 1;

                    ($token.to_string().into_boxed_str(), current_token_id)
                });

            if *token_id as usize >= $self.token_id_to_freq.len() {
                $self.token_id_to_freq.push((*token_id, 1));
                $self.token_id_to_token
                    .push($token.to_string().into_boxed_str());
                $self.token_id_to_pl.push(PostingList::default());
            } else {
                $self.token_id_to_freq[*token_id as usize].1 += 1;
            }

            *token_id
            // Some(*token_id)
        }
    };
}

macro_rules! analyze_common_tokens_sequence {
    ($label:lifetime, $token_id_label:lifetime, $self:ident, $begin_pos:ident, $sequence:ident, $token_id_to_positions:ident) => {
        $label: {
            if $sequence.len() <= 1 {
                break $label;
            }

            for i in 0..($sequence.len() - 1) {
                let b = i + 2;
                let e = ($sequence.len() + 1).min(i + MAX_SEQ_LEN + 1);
                for j in b..e {
                    let token: String = $sequence[i..j]
                        .iter()
                        .map(|token_id| $self.token_id_to_token[*token_id as usize].as_ref())
                        .intersperse(" ")
                        .collect();
                    let token = token.as_str();
                    let token_id = get_token_id!($token_id_label, $self, token);

                    $token_id_to_positions
                        .entry(token_id)
                        .or_default()
                        .push(($begin_pos + i) as u32);
                }
            }
        }
    };
}

#[derive(Debug)]
pub enum CommonTokens {
    List(Vec<String>),
    FixedNum(u32),
    Percentage(f64),
}

#[derive(Debug)]
struct InnerIndexer<D>
where
    D: for<'a> Serialize<DefaultSerializer<'a, AlignedVec, rkyv::rancor::Error>> + Archive,
{
    next_token_id: u32,
    token_to_token_id: AHashMap<Box<str>, u32>,

    // this 3 containers are in sync
    token_id_to_freq: Vec<(u32, u32)>,
    token_id_to_pl: Vec<PostingList>,
    token_id_to_token: Vec<Box<str>>,

    // this 3 containers are in sync
    doc_ids: Vec<u32>,
    documents: Vec<D>,
    token_id_reprs: Vec<Vec<u32>>,
}

impl<D> InnerIndexer<D>
where
    D: for<'a> Serialize<DefaultSerializer<'a, AlignedVec, rkyv::rancor::Error>>
        + Archive
        + 'static,
{
    fn new(documents_per_shard: Option<u32>, avg_document_tokens: Option<u32>) -> Self {
        match documents_per_shard {
            Some(documents_per_shard) => match avg_document_tokens {
                Some(avg_document_tokens) => {
                    // Heap's Law
                    let len = (1.705
                        * (documents_per_shard as f64 * avg_document_tokens as f64).powf(0.786))
                    .ceil() as usize;
                    Self {
                        next_token_id: 0,
                        token_to_token_id: AHashMap::with_capacity(len),
                        token_id_to_freq: Vec::with_capacity(len),
                        token_id_to_pl: Vec::with_capacity(len),
                        token_id_to_token: Vec::with_capacity(len),
                        token_id_reprs: Vec::with_capacity(len),
                        doc_ids: Vec::with_capacity(documents_per_shard as usize),
                        documents: Vec::with_capacity(documents_per_shard as usize),
                    }
                }
                None => Self {
                    next_token_id: 0,
                    token_to_token_id: AHashMap::new(),
                    token_id_to_freq: Vec::new(),
                    token_id_to_pl: Vec::new(),
                    token_id_to_token: Vec::new(),
                    token_id_reprs: Vec::new(),
                    doc_ids: Vec::with_capacity(documents_per_shard as usize),
                    documents: Vec::with_capacity(documents_per_shard as usize),
                },
            },
            None => Self {
                next_token_id: 0,
                token_to_token_id: AHashMap::new(),
                token_id_to_freq: Vec::new(),
                token_id_to_pl: Vec::new(),
                token_id_to_token: Vec::new(),
                token_id_reprs: Vec::new(),
                doc_ids: Vec::new(),
                documents: Vec::new(),
            },
        }
    }

    fn push(&mut self, doc_id: u32, content: &str, doc: D) {
        self.doc_ids.push(doc_id);
        self.documents.push(doc);

        let mut token_id_repr = Vec::new();
        self.index_doc(content, doc_id, &mut token_id_repr);

        self.token_id_reprs.push(token_id_repr);
    }

    fn index_doc(&mut self, content: &str, doc_id: u32, token_id_repr: &mut Vec<u32>) {
        let mut token_id_to_positions: FxHashMap<u32, Vec<u32>> = FxHashMap::new();
        let content = normalize(content);
        for (pos, token) in tokenize(&content).enumerate().take(MAX_VALUE as usize) {
            let token_id = get_token_id!('a, self, token);

            token_id_to_positions
                .entry(token_id)
                .or_default()
                .push(pos as u32);
            token_id_repr.push(token_id);
        }

        for (token_id, positions) in token_id_to_positions {
            if token_id == TOKEN_ID_TOO_LONG {
                continue;
            }

            self.token_id_to_pl[token_id as usize]
                .push_unchecked(doc_id, Roaringish::from_positions_sorted(positions));
        }
    }

    fn flush(
        mut self,
        path: &Path,
        db_size: usize,
        shard_id: u32,
        common_tokens: &Option<CommonTokens>,
    ) -> DB<D> {
        let b = std::time::Instant::now();
        let path = path.join(shard_id.to_string());

        let db = DB::truncate(&path, db_size);

        let common_token_ids = match common_tokens {
            Some(common_tokens) => self.generate_common_tokens(common_tokens),
            None => FxHashSet::new(),
        };

        let mut rwtxn = db.env.write_txn().unwrap();

        db.write_token_to_token_id(&mut rwtxn, &self.token_to_token_id);

        db.write_postings_list(&mut rwtxn, &self.token_id_to_pl);

        db.write_doc_id_to_document(&mut rwtxn, &self.doc_ids, &self.documents);

        db.write_common_token_ids(&mut rwtxn, &common_token_ids, &self.token_id_to_token);

        rwtxn.commit().unwrap();

        println!("Flushed shard: {shard_id} in {:?}", b.elapsed());

        db
    }

    fn generate_common_tokens(&mut self, common_tokens: &CommonTokens) -> FxHashSet<u32> {
        println!("before: {}", self.token_id_to_pl.len());
        let common_token_ids: FxHashSet<_> = match common_tokens {
            CommonTokens::List(tokens) => tokens
                .iter()
                .filter_map(|t| self.token_to_token_id.get(t.as_str()).copied())
                .collect(),
            CommonTokens::FixedNum(max) => {
                let max = (*max as usize).min(self.token_id_to_freq.len());
                self.token_id_to_freq
                    .sort_unstable_by_key(|(_, freq)| Reverse(*freq));
                self.token_id_to_freq[0..max]
                    .iter()
                    .map(|(token_id, _)| *token_id)
                    .collect()
            }
            CommonTokens::Percentage(p) => {
                let max = (self.token_id_to_freq.len() as f64 * *p) as usize;
                self.token_id_to_freq
                    .sort_unstable_by_key(|(_, freq)| Reverse(*freq));
                self.token_id_to_freq[0..max]
                    .iter()
                    .map(|(token_id, _)| *token_id)
                    .collect()
            }
        };

        // for id in common_token_ids.iter(){
        //     println!("{id} -> {}", self.token_id_to_token[*id as usize]);
        // }
        // for t in self.token_id_to_token.iter().enumerate() {
        //     println!("{t:?}");
        // }

        let mut sequence = Vec::new();
        for (token_id_repr, doc_id) in self.token_id_reprs.iter().zip(self.doc_ids.iter()) {
            let mut token_id_to_positions: FxHashMap<u32, Vec<u32>> = FxHashMap::new();
            sequence.clear();
            let mut begin_pos = 0;
            for (pos, token_id) in token_id_repr.iter().enumerate() {
                if common_token_ids.contains(&token_id) {
                    sequence.push(*token_id);
                    continue;
                }

                if sequence.len() <= 1 {
                    sequence.clear();
                    begin_pos = pos + 1;
                    continue;
                }

                analyze_common_tokens_sequence!('a, 'b, self, begin_pos, sequence, token_id_to_positions);

                sequence.clear();
                begin_pos = pos + 1;
            }

            analyze_common_tokens_sequence!('a, 'b, self, begin_pos, sequence, token_id_to_positions);

            for (token_id, positions) in token_id_to_positions {
                if token_id == TOKEN_ID_TOO_LONG {
                    continue;
                }

                self.token_id_to_pl[token_id as usize]
                    .push_unchecked(*doc_id, Roaringish::from_positions_sorted(positions));
            }
        }

        println!("after: {}", self.token_id_to_pl.len());
        return common_token_ids;
    }
}

pub struct Indexer<'a, D>
where
    D: for<'b> Serialize<DefaultSerializer<'b, AlignedVec, rkyv::rancor::Error>> + Archive,
{
    path: &'a Path,
    shard_db_size: usize,
    next_doc_id: u32,
    docs_per_shard: Option<u32>,
    indexer: Option<InnerIndexer<D>>,
    shards: Vec<DB<D>>,
    common_tokens: Option<CommonTokens>,
    avg_document_tokens: Option<u32>,
}

impl<'a, D> Indexer<'a, D>
where
    D: for<'b> Serialize<DefaultSerializer<'b, AlignedVec, rkyv::rancor::Error>>
        + Archive
        + 'static,
{
    pub fn new(
        path: &'a Path,
        shard_db_size: usize,
        docs_per_shard: Option<u32>,
        common_tokens: Option<CommonTokens>,
        avg_document_tokens: Option<u32>,
    ) -> Self {
        Self {
            path,
            shard_db_size,
            next_doc_id: 0,
            docs_per_shard,
            indexer: Some(InnerIndexer::new(docs_per_shard, avg_document_tokens)),
            shards: Vec::new(),
            common_tokens,
            avg_document_tokens,
        }
    }

    pub fn index<S: AsRef<str>, I: IntoIterator<Item = (S, D)>>(&mut self, docs: I) -> u32 {
        let mut num_docs = 0;
        for (content, doc) in docs {
            num_docs += 1;
            let doc_id = self.next_doc_id;
            self.next_doc_id += 1;

            let indexer = self.indexer.get_or_insert_with(|| {
                InnerIndexer::new(self.docs_per_shard, self.avg_document_tokens)
            });
            indexer.push(doc_id, content.as_ref(), doc);

            if let Some(docs_per_shard) = self.docs_per_shard {
                if self.next_doc_id % docs_per_shard == 0 {
                    // this should neve fail
                    let indexer = self.indexer.take().unwrap();
                    let b = std::time::Instant::now();
                    let db = indexer.flush(
                        self.path,
                        self.shard_db_size,
                        self.shards.len() as u32,
                        &self.common_tokens,
                    );
                    println!("Outter: {:?}", b.elapsed());
                    self.shards.push(db);
                }
            }
        }

        num_docs
    }

    pub fn flush(mut self) -> Searcher<D> {
        match self.indexer {
            Some(indexer) => {
                let b = std::time::Instant::now();
                let db = indexer.flush(
                    self.path,
                    self.shard_db_size,
                    self.shards.len() as u32,
                    &self.common_tokens,
                );
                println!("Outter: {:?}", b.elapsed());
                self.shards.push(db);
            }
            None => {}
        }

        Searcher { shards: self.shards.into_boxed_slice() }
    }
}

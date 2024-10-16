use std::{cmp::Reverse, path::PathBuf, sync::atomic::{AtomicU32, Ordering}};

use ahash::{AHashMap, HashMapExt, HashSetExt};
use fxhash::{FxHashMap, FxHashSet};
use roaring::RoaringBitmap;

use crate::{db::DB, document::Document, utils::{normalize, tokenize, MAX_SEQ_LEN}};

#[derive(Debug)]
pub struct Indexer<'a, 'b> {
    next_doc_id: &'a AtomicU32,

    next_token_id: u32,
    token_to_token_id: AHashMap<Box<str>, u32>,
    token_id_to_freq: Vec<(u32, u32)>,
    token_id_doc_id_to_positions: FxHashMap<(u32, u32), Vec<u32>>,

    token_id_to_token: Vec<Box<str>>,

    // this 2 containers are in sync
    doc_ids: Vec<u32>,
    documents: Vec<Document>,

    db: &'b DB,
}

impl<'a, 'b> Indexer<'a, 'b> {
    pub fn new(next_doc_id: &'a AtomicU32, db: &'b DB) -> Self {
        Self {
            next_doc_id,
            token_to_token_id: AHashMap::new(),
            token_id_to_freq: Vec::new(),
            next_token_id: 0,
            doc_ids: Vec::new(),
            documents: Vec::new(),
            token_id_doc_id_to_positions: FxHashMap::new(),
            token_id_to_token: Vec::new(),
            db,
        }
    }

    fn get_token_id(&mut self, token: &str) -> Option<u32> {
        if token.as_bytes().len() > 511 {
            return None;
        }

        let (_, token_id) = self
            .token_to_token_id
            .raw_entry_mut()
            .from_key(token)
            .or_insert_with(|| {
                let current_token_id = self.next_token_id;
                self.next_token_id += 1;

                (token.to_string().into_boxed_str(), current_token_id)
            });

        if *token_id as usize >= self.token_id_to_freq.len() {
            self.token_id_to_freq.push((*token_id, 1));
            self.token_id_to_token
                .push(token.to_string().into_boxed_str());
        } else {
            self.token_id_to_freq[*token_id as usize].1 += 1;
        }

        Some(*token_id)
    }

    fn index_doc(&mut self, content: &str, current_doc_id: u32, token_id_repr: &mut Vec<u32>) {
        let content = normalize(content);
        for (pos, token) in tokenize(&content).enumerate() {
            let Some(token_id) = self.get_token_id(token) else {
                continue;
            };

            self.token_id_doc_id_to_positions
                .entry((token_id, current_doc_id))
                .or_default()
                .push(pos as u32);

            token_id_repr.push(token_id);
        }
    }

    fn analyze_common_tokens_sequence(
        &mut self,
        doc_id: u32,
        begin_pos: usize,
        sequence: &mut Vec<u32>,
    ) {
        if sequence.len() <= 1 {
            return;
        }

        for i in 0..(sequence.len() - 1) {
            let b = i + 2;
            let e = (sequence.len() + 1).min(i + MAX_SEQ_LEN + 1);
            for j in b..e {
                let token: String = sequence[i..j]
                    .iter()
                    .map(|token_id| self.token_id_to_token[*token_id as usize].as_ref())
                    .intersperse(" ")
                    .collect();
                let Some(token_id) = self.get_token_id(&token) else {
                    break;
                };

                self.token_id_doc_id_to_positions
                    .entry((token_id, doc_id))
                    .or_default()
                    .push((begin_pos + i) as u32);
            }
        }
    }

    fn generate_common_tokens(&mut self, token_id_reprs: Vec<Vec<u32>>) -> FxHashSet<u32> {
        let max = (self.token_id_to_freq.len() as f64 * 0.00002f64) as usize;
        self.token_id_to_freq
            .sort_unstable_by_key(|(_, freq)| Reverse(*freq));
        let common_token_ids: FxHashSet<_> = self.token_id_to_freq[0..max]
            .iter()
            .map(|(token_id, _)| *token_id)
            .collect();

        // I don't want to clone, but the compiler is forcing me...
        let doc_ids = self.doc_ids.clone();

        let mut sequence = Vec::new();
        for (token_id_repr, doc_id) in token_id_reprs.into_iter().zip(doc_ids.into_iter()) {
            sequence.clear();
            let mut begin_pos = 0;
            for (pos, token_id) in token_id_repr.into_iter().enumerate() {
                if common_token_ids.contains(&token_id) {
                    sequence.push(token_id);
                    continue;
                }

                if sequence.len() <= 1 {
                    sequence.clear();
                    begin_pos = pos + 1;
                    continue;
                }

                self.analyze_common_tokens_sequence(doc_id, begin_pos, &mut sequence);

                sequence.clear();
                begin_pos = pos + 1;
            }

            self.analyze_common_tokens_sequence(doc_id, begin_pos, &mut sequence);
        }

        return common_token_ids;
    }

    pub fn index(mut self, files: &[PathBuf]) -> (u32, u32) {
        let mut token_id_reprs = Vec::new();
        let mut last_doc_id = 0;
        let mut docs_in_shard = 0;
        for file in files.iter() {
            let docs = std::fs::read_to_string(file).unwrap();
            for str_doc in docs.lines() {
                docs_in_shard += 1;
                let document: Document = serde_json::from_str(str_doc).unwrap();

                let doc_id = self.next_doc_id.fetch_add(1, Ordering::Relaxed);
                last_doc_id = doc_id;
        
                self.doc_ids.push(doc_id);
                self.documents.push(document);
                token_id_reprs.push(Vec::new());
                let token_id_repr = token_id_reprs.last_mut().unwrap();

                let Some(content) = self.documents.last().unwrap().content.clone() else {
                    continue;
                };

                self.index_doc(&content, doc_id, token_id_repr);
            }
        }

        let common_token_ids = self.generate_common_tokens(token_id_reprs);
        self.flush(common_token_ids, files, docs_in_shard, last_doc_id);
        (docs_in_shard, last_doc_id)
    }

    fn flush(self, common_token_ids: FxHashSet<u32>, files: &[PathBuf], docs_in_shard: u32, last_doc_id: u32) {
        let mut rwtxn = self.db.env.write_txn().unwrap();

        self.db
            .write_token_to_token_id(&mut rwtxn, self.token_to_token_id);

        self.db.write_postings_list(
            &mut rwtxn,
            self.token_id_doc_id_to_positions,
            self.token_id_to_token.len(),
        );

        self.db
            .write_doc_id_to_document(&mut rwtxn, self.doc_ids, self.documents);

        self.db
            .write_common_token_ids(&mut rwtxn, common_token_ids, &self.token_id_to_token);

        rwtxn.commit().unwrap();

        self.db.write_files(files, docs_in_shard, last_doc_id);
    }
}
#![feature(hash_raw_entry)]
#![feature(array_windows)]
#![feature(iter_intersperse)]
#![feature(core_intrinsics)]
#![feature(debug_closure_helpers)]
#![feature(maybe_uninit_fill)]
#![feature(maybe_uninit_write_slice)]
#![feature(vec_push_within_capacity)]
#![feature(trivial_bounds)]
#![feature(portable_simd)]
#![feature(stdarch_x86_avx512)]
#![feature(avx512_target_feature)]
#![feature(maybe_uninit_uninit_array)]
#![feature(allocator_api)]
#![feature(str_as_str)]
#![feature(pointer_is_aligned_to)]
#![feature(array_chunks)]
#![feature(maybe_uninit_slice)]
#![feature(new_range_api)]

mod codecs;
mod db;
mod indexer;
mod roaringish;
mod searcher;
mod utils;
mod allocator;
mod decreasing_window_iter;

pub use db::{Stats, DB};
pub use indexer::Indexer;
pub use indexer::CommonTokens;

pub use roaringish::intersect::naive;

// #[cfg(all(
//     target_feature = "avx512f",
//     target_feature = "avx512bw",
//     target_feature = "avx512vl",
//     target_feature = "avx512vbmi2",
//     target_feature = "avx512dq",
// ))]
pub use roaringish::intersect::simd;

pub use roaringish::intersect::Intersect;
pub use roaringish::RoaringishPacked;
pub use roaringish::BorrowRoaringishPacked;
pub use searcher::Searcher;
pub use utils::{normalize, tokenize};
pub use allocator::Aligned64;

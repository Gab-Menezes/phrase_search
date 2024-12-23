#![feature(allocator_api)]

#![no_main]

use libfuzzer_sys::fuzz_target;
use phrase_search::RoaringishPacked;
use phrase_search::BorrowRoaringishPacked;
use phrase_search::naive::NaiveIntersect;
use phrase_search::Intersect;
use phrase_search::simd::SimdIntersect;
use phrase_search::Aligned64;
use phrase_search::Stats;

fn compare(lhs: &(Vec<u64, Aligned64>, Vec<u64, Aligned64>), rhs: &(Vec<u64, Aligned64>, Vec<u64, Aligned64>)) {
    assert_eq!(lhs.0, rhs.0);

    assert!(lhs.1.len() <= rhs.1.len());
    assert_eq!(lhs.1, rhs.1[..lhs.1.len()]);

    // assert_eq!(lhs.1, rhs.1);
}

fuzz_target!(|r: (RoaringishPacked, RoaringishPacked, u16)| {
    let stats = Stats::default();

    let lhs = BorrowRoaringishPacked::new(&r.0);
    let rhs = BorrowRoaringishPacked::new(&r.1);

    let l = (r.2 % 4) + 1;

    let naive = NaiveIntersect::intersect::<true>(lhs, rhs, l, &stats);
    let simd = SimdIntersect::intersect::<true>(lhs, rhs, l, &stats);
    compare(&naive, &simd);

    let naive = NaiveIntersect::intersect::<false>(lhs, rhs, l);
    let simd = SimdIntersect::intersect::<false>(lhs, rhs, l);
    compare(&naive, &simd);
});

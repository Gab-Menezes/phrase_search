#![no_main]

use libfuzzer_sys::fuzz_target;
use phrase_search::RoaringishPacked;
use phrase_search::BorrowRoaringishPacked;
use phrase_search::naive::NaiveIntersect;
use phrase_search::naive::UnrolledNaiveIntersect;
use phrase_search::Intersect;
use phrase_search::gallop::GallopIntersect;
use phrase_search::simd::SimdIntersectCMOV;
use phrase_search::vp2intersectq::Vp2Intersectq;

fuzz_target!(|r: (RoaringishPacked, RoaringishPacked)| {
    let lhs = BorrowRoaringishPacked::new(&r.0);
    let rhs = BorrowRoaringishPacked::new(&r.1);
    
    let naive = NaiveIntersect::intersect::<true>(&lhs, &rhs);
    // let unrolled_naive = UnrolledNaiveIntersect::intersect::<true>(&lhs, &rhs);
    // let gallop = GallopIntersect::intersect::<true>(&lhs, &rhs);
    // let simd_cmov = SimdIntersectCMOV::intersect::<true>(&lhs, &rhs);
    let vp2intersectq = Vp2Intersectq::intersect::<true>(&lhs, &rhs);
    // assert_eq!(naive, gallop);
    // assert_eq!(naive, unrolled_naive);
    assert_eq!(naive, vp2intersectq);
});
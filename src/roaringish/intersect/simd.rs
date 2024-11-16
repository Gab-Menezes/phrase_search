#[allow(unused_imports)]
use std::{
    arch::x86_64::__m512i,
    mem::MaybeUninit,
    simd::{cmp::SimdPartialOrd, Simd},
};
use std::{
    arch::x86_64::{
        _mm512_load_epi64, _mm512_mask_compressstoreu_epi64, _mm512_maskz_compress_epi64,
        _mm512_storeu_epi64, _mm_load_si128, _mm_loadu_epi16, _mm_mask_compressstoreu_epi16,
        _mm_maskz_compress_epi16, _mm_storeu_epi16,
    },
    simd::num::SimdUint, sync::atomic::Ordering::Relaxed,
};

use crate::{roaringish::{
    clear_values_simd, unpack_values_simd, Aligned64, BorrowRoaringishPacked, ADD_ONE_GROUP,
}, Stats};

use super::naive::NaiveIntersect;
use super::{private::IntersectSeal, Intersect};
use crate::roaringish::Aligned;

const N: usize = 8;

#[cfg(target_feature = "avx512vp2intersect")]
#[inline(always)]
unsafe fn vp2intersectq(a: __m512i, b: __m512i) -> (u8, u8) {
    use std::arch::{asm, x86_64::__mmask8};

    let mut mask0: __mmask8;
    let mut mask1: __mmask8;
    asm!(
        "vp2intersectq k2, {0}, {1}",
        in(zmm_reg) a,
        in(zmm_reg) b,
        out("k2") mask0,
        out("k3") mask1,
        options(pure, nomem, nostack),
    );

    (mask0, mask1)
}

#[cfg(not(target_feature = "avx512vp2intersect"))]
#[inline(always)]
unsafe fn vp2intersectq(a: __m512i, b: __m512i) -> (u8, u8) {
    use std::arch::x86_64::{
        _mm512_alignr_epi32, _mm512_cmpeq_epi64_mask, _mm512_shuffle_epi32, _MM_PERM_BADC,
    };

    let a1 = _mm512_alignr_epi32(a, a, 4);
    let a2 = _mm512_alignr_epi32(a, a, 8);
    let a3 = _mm512_alignr_epi32(a, a, 12);

    let b1 = _mm512_shuffle_epi32(b, _MM_PERM_BADC);

    let m00 = _mm512_cmpeq_epi64_mask(a, b);
    let m01 = _mm512_cmpeq_epi64_mask(a, b1);
    let m10 = _mm512_cmpeq_epi64_mask(a1, b);
    let m11 = _mm512_cmpeq_epi64_mask(a1, b1);
    let m20 = _mm512_cmpeq_epi64_mask(a2, b);
    let m21 = _mm512_cmpeq_epi64_mask(a2, b1);
    let m30 = _mm512_cmpeq_epi64_mask(a3, b);
    let m31 = _mm512_cmpeq_epi64_mask(a3, b1);

    let mask0 = m00
        | m01
        | (m10 | m11).rotate_left(2)
        | (m20 | m21).rotate_left(4)
        | (m30 | m31).rotate_left(6);

    let m0 = m00 | m10 | m20 | m30;
    let m1 = m01 | m11 | m21 | m31;
    let mask1 = m0 | ((0x55 & m1) << 1) | ((m1 >> 1) & 0x55);

    (mask0, mask1)
}

#[inline(always)]
unsafe fn analyze_msb(
    lhs_pack: Simd<u64, N>,
    lhs_values: Simd<u16, N>,
    msb_packed_result: &mut [MaybeUninit<u64>],
    j: &mut usize,
    msb_mask: Simd<u16, N>,
) {
    let mask = (lhs_values & msb_mask).simd_gt(Simd::splat(0)).to_bitmask() as u8;
    let pack_plus_one: Simd<u64, N> = lhs_pack + Simd::splat(ADD_ONE_GROUP);
    unsafe {
        // TODO: avoid compressstore on zen4
        _mm512_mask_compressstoreu_epi64(
            msb_packed_result.as_mut_ptr().add(*j) as *mut _,
            mask,
            pack_plus_one.into(),
        );
    }
    *j += mask.count_ones() as usize;
}

#[inline(always)]
fn rotl_u16(a: Simd<u16, N>, i: u16) -> Simd<u16, N> {
    let p0 = a << i;
    let p1 = a >> (16 - i);
    p0 | p1
}

pub struct SimdIntersect;
impl IntersectSeal for SimdIntersect {}

impl Intersect for SimdIntersect {
    #[inline(always)]
    fn inner_intersect<const FIRST: bool>(
        lhs: BorrowRoaringishPacked<'_, Aligned>,
        rhs: BorrowRoaringishPacked<'_, Aligned>,

        lhs_i: &mut usize,
        rhs_i: &mut usize,

        packed_result: &mut Box<[MaybeUninit<u64>], Aligned64>,
        i: &mut usize,

        msb_packed_result: &mut Box<[MaybeUninit<u64>], Aligned64>,
        j: &mut usize,

        lhs_len: u16,
        msb_mask: u16,
        lsb_mask: u16,

        stats: &Stats,
    ) {
        let b = std::time::Instant::now();
        
        let simd_msb_mask = Simd::splat(msb_mask);
        let simd_lsb_mask = Simd::splat(lsb_mask);

        let end_lhs = lhs.0.len() / N * N;
        let end_rhs = rhs.0.len() / N * N;
        let lhs_packed = unsafe { lhs.0.get_unchecked(..end_lhs) };
        let rhs_packed = unsafe { rhs.0.get_unchecked(..end_rhs) };
        assert_eq!(lhs_packed.len() % N, 0);
        assert_eq!(rhs_packed.len() % N, 0);

        let mut need_to_analyze_msb = false;

        while *lhs_i < lhs_packed.len() && *rhs_i < rhs_packed.len() {
            let (lhs_pack, rhs_pack) = unsafe {
                let lhs_pack = _mm512_load_epi64(lhs_packed.as_ptr().add(*lhs_i) as *const _);
                let rhs_pack = _mm512_load_epi64(rhs_packed.as_ptr().add(*rhs_i) as *const _);
                (lhs_pack, rhs_pack)
            };

            let lhs_doc_id_group = clear_values_simd(lhs_pack.into());
            let lhs_values = unpack_values_simd(lhs_pack.into());

            let rhs_doc_id_group = clear_values_simd(rhs_pack.into());
            let rhs_values = unpack_values_simd(rhs_pack.into());

            let (lhs_mask, rhs_mask) =
                unsafe { vp2intersectq(lhs_doc_id_group.into(), rhs_doc_id_group.into()) };

            unsafe {
                let doc_id_group_compress: Simd<u64, N> =
                    _mm512_maskz_compress_epi64(lhs_mask, lhs_doc_id_group.into()).into();
                let lhs_values_compress: Simd<u16, N> =
                    _mm_maskz_compress_epi16(lhs_mask, lhs_values.into()).into();
                let rhs_values_compress: Simd<u16, N> =
                    _mm_maskz_compress_epi16(rhs_mask, rhs_values.into()).into();

                if FIRST {
                    let intersection =
                        ((lhs_values_compress << lhs_len) & rhs_values_compress).cast();
                    _mm512_storeu_epi64(
                        packed_result.as_mut_ptr().add(*i) as *mut _,
                        (doc_id_group_compress | intersection).into(),
                    );
                } else {
                    let intersection = (rotl_u16(lhs_values_compress, lhs_len)
                        & simd_lsb_mask
                        & rhs_values_compress)
                        .cast();
                    _mm512_storeu_epi64(
                        packed_result.as_mut_ptr().add(*i) as *mut _,
                        (doc_id_group_compress | intersection).into(),
                    );
                }

                *i += lhs_mask.count_ones() as usize;
            }

            let lhs_last = unsafe { *lhs_packed.get_unchecked(*lhs_i + N - 1) };
            let rhs_last = unsafe { *rhs_packed.get_unchecked(*rhs_i + N - 1) };

            if FIRST {
                if lhs_last <= rhs_last {
                    unsafe {
                        analyze_msb(
                            lhs_pack.into(),
                            lhs_values,
                            msb_packed_result,
                            j,
                            simd_msb_mask,
                        );
                    }
                    *lhs_i += N;
                }
            } else {
                *lhs_i += N * (lhs_last <= rhs_last) as usize;
            }
            *rhs_i += N * (rhs_last <= lhs_last) as usize;
            need_to_analyze_msb = rhs_last < lhs_last;
        }

        if FIRST && need_to_analyze_msb && !(*lhs_i < lhs.0.len() && *rhs_i < rhs.0.len()) {
            unsafe {
                let lhs_pack = _mm512_load_epi64(lhs_packed.as_ptr().add(*lhs_i) as *const _);
                let lhs_values = unpack_values_simd(lhs_pack.into());
                analyze_msb(
                    lhs_pack.into(),
                    lhs_values,
                    msb_packed_result,
                    j,
                    simd_msb_mask,
                );
            };
        }

        if FIRST {
            stats
            .first_intersect_simd
            .fetch_add(b.elapsed().as_micros() as u64, Relaxed);
        } else {
            stats
            .second_intersect_simd
            .fetch_add(b.elapsed().as_micros() as u64, Relaxed);
        }

        NaiveIntersect::inner_intersect::<FIRST>(
            lhs,
            rhs,
            lhs_i,
            rhs_i,
            packed_result,
            i,
            msb_packed_result,
            j,
            lhs_len,
            msb_mask,
            lsb_mask,

            stats
        );
    }

    fn intersection_buffer_size(
        lhs: BorrowRoaringishPacked<'_, Aligned>,
        rhs: BorrowRoaringishPacked<'_, Aligned>,
    ) -> usize {
        lhs.0.len().min(rhs.0.len()) + 1 + N
    }
}

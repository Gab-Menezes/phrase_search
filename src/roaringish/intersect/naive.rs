use std::{mem::MaybeUninit, sync::atomic::Ordering::Relaxed};

use crate::{
    allocator::Aligned64,
    roaringish::{clear_values, unpack_values, Aligned, BorrowRoaringishPacked, ADD_ONE_GROUP}, Stats,
};

use super::{private::IntersectSeal, Intersect};

pub struct NaiveIntersect;
impl IntersectSeal for NaiveIntersect {}

impl Intersect for NaiveIntersect {
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
        
        while *lhs_i < lhs.0.len() && *rhs_i < rhs.0.len() {
            let lhs_packed = unsafe { *lhs.0.get_unchecked(*lhs_i) };
            let lhs_doc_id_group = clear_values(lhs_packed);
            let lhs_values = unpack_values(lhs_packed);

            let rhs_packed = unsafe { *rhs.0.get_unchecked(*rhs_i) };
            let rhs_doc_id_group = clear_values(rhs_packed);
            let rhs_values = unpack_values(rhs_packed);

            if lhs_doc_id_group == rhs_doc_id_group {
                unsafe {
                    if FIRST {
                        let intersection = (lhs_values << lhs_len) & rhs_values;
                        packed_result
                            .get_unchecked_mut(*i)
                            .write(lhs_doc_id_group | intersection as u64);

                        msb_packed_result
                            .get_unchecked_mut(*j)
                            .write(lhs_packed + ADD_ONE_GROUP);

                        *j += (lhs_values & msb_mask > 0) as usize;
                    } else {
                        let intersection =
                            lhs_values.rotate_left(lhs_len as u32) & lsb_mask & rhs_values;
                        packed_result
                            .get_unchecked_mut(*i)
                            .write(lhs_doc_id_group | intersection as u64);
                    }
                }
                *i += 1;
                *lhs_i += 1;
                *rhs_i += 1;
            } else if lhs_doc_id_group > rhs_doc_id_group {
                *rhs_i += 1;
            } else {
                if FIRST {
                    unsafe {
                        msb_packed_result
                            .get_unchecked_mut(*j)
                            .write(lhs_packed + ADD_ONE_GROUP);
                        *j += (lhs_values & msb_mask > 0) as usize;
                    }
                }
                *lhs_i += 1;
            }
        }

        if FIRST {
            stats
            .first_intersect_naive
            .fetch_add(b.elapsed().as_micros() as u64, Relaxed);
        } else {
            stats
            .second_intersect_naive
            .fetch_add(b.elapsed().as_micros() as u64, Relaxed);
        }
    }

    fn intersection_buffer_size(
        lhs: BorrowRoaringishPacked<'_, Aligned>,
        rhs: BorrowRoaringishPacked<'_, Aligned>,
    ) -> usize {
        lhs.0.len().min(rhs.0.len())
    }
}

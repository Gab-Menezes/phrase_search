use std::mem::MaybeUninit;

use crate::{allocator::Aligned64, roaringish::BorrowRoaringishPacked};

use super::{private::IntersectSeal, Intersect};

pub struct NaiveIntersect;
impl IntersectSeal for NaiveIntersect {}

impl Intersect for NaiveIntersect {
    fn inner_intersect<const FIRST: bool>(
        lhs: &BorrowRoaringishPacked,
        rhs: &BorrowRoaringishPacked,

        lhs_i: &mut usize,
        rhs_i: &mut usize,

        doc_id_groups_result: &mut Box<[MaybeUninit<u64>], Aligned64>,
        values_result: &mut Box<[MaybeUninit<u16>], Aligned64>,
        i: &mut usize,

        msb_doc_id_groups_result: &mut Box<[MaybeUninit<u64>], Aligned64>,
        j: &mut usize,
    ) {
        while *lhs_i < lhs.doc_id_groups.len() && *rhs_i < rhs.doc_id_groups.len() {
            let lhs_doc_id_groups = unsafe { *lhs.doc_id_groups.get_unchecked(*lhs_i) };
            let rhs_doc_id_groups = unsafe { *rhs.doc_id_groups.get_unchecked(*rhs_i) };

            if lhs_doc_id_groups == rhs_doc_id_groups {
                unsafe {
                    doc_id_groups_result
                        .get_unchecked_mut(*i)
                        .write(lhs_doc_id_groups);
                    let rhs = *rhs.values.get_unchecked(*rhs_i);
                    if FIRST {
                        let lhs = *lhs.values.get_unchecked(*lhs_i);
                        values_result.get_unchecked_mut(*i).write((lhs << 1) & rhs);

                        msb_doc_id_groups_result
                            .get_unchecked_mut(*j)
                            .write(lhs_doc_id_groups + 1);
                        *j += (lhs & 0x8000 > 1) as usize;
                    } else {
                        values_result.get_unchecked_mut(*i).write(1 & rhs);
                    }
                }
                *i += 1;
                *lhs_i += 1;
                *rhs_i += 1;
            } else if lhs_doc_id_groups > rhs_doc_id_groups {
                *rhs_i += 1;
            } else {
                if FIRST {
                    unsafe {
                        let lhs = *lhs.values.get_unchecked(*lhs_i);
                        msb_doc_id_groups_result
                            .get_unchecked_mut(*j)
                            .write(lhs_doc_id_groups + 1);
                        *j += (lhs & 0x8000 > 1) as usize;
                    }
                }
                *lhs_i += 1;
            }
        }
    }

    fn intersection_buffer_size(
        lhs: &BorrowRoaringishPacked,
        rhs: &BorrowRoaringishPacked,
    ) -> usize {
        lhs.doc_id_groups.len().min(rhs.doc_id_groups.len())
    }
}

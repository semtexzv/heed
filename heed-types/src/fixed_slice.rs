use std::borrow::Cow;
use std::ptr;

use heed_traits::{BytesDecode, BytesEncode};
use zerocopy::{AsBytes, FromBytes, LayoutVerified};

pub struct FixedSlice<T, const N: usize>(std::marker::PhantomData<T>);

impl<'a, T: 'a, const N: usize> BytesEncode<'a> for FixedSlice<T, N>
where
    T: AsBytes,
{
    type EItem = [T; N];

    fn bytes_encode(item: &'a Self::EItem) -> Option<Cow<[u8]>> {
        Some(Cow::Borrowed(<[T] as AsBytes>::as_bytes(item)))
    }
}

impl<T: 'static, const N: usize> BytesDecode for FixedSlice<T, N>
where
    [T; N]: FromBytes + Default + Copy,
{
    type DItem = [T; N];

    fn bytes_decode(bytes: &[u8]) -> Option<Self::DItem> {
        match LayoutVerified::<_, [T; N]>::new(bytes) {
            Some(v) => Some(v.into_ref().clone()),
            None => {
                assert_eq!(bytes.len(), std::mem::size_of::<[T; N]>());
                let mut out = <[T; N] as Default>::default();

                unsafe {
                    let dst = &mut out as *mut [T; N] as *mut u8;
                    ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
                }
                Some(out)
            }
        }
    }
}

unsafe impl<T, const N: usize> Send for FixedSlice<T, N> {}

unsafe impl<T, const N: usize> Sync for FixedSlice<T, N> {}

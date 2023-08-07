use std::borrow::Cow;

use heed_traits::{BytesDecode, BytesEncode};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Describes a type that is [`Serialize`]/[`Deserialize`] and uses `bincode` to do so.
///
/// It can borrow bytes from the original slice.
pub struct SerdeBincode<T>(std::marker::PhantomData<T>);

impl<'a, T: 'a> BytesEncode<'a> for SerdeBincode<T>
where
    T: Serialize,
{
    type EItem = T;

    fn bytes_encode(item: &'a Self::EItem) -> Option<Cow<[u8]>> {
        bincode::serialize(item).map(Cow::Owned).ok()
    }
}

impl<T: 'static> BytesDecode for SerdeBincode<T>
where
    T: DeserializeOwned,
{
    type DItem = T;

    fn bytes_decode(bytes: &[u8]) -> Option<Self::DItem> {
        bincode::deserialize(bytes).ok()
    }
}

unsafe impl<T> Send for SerdeBincode<T> {}

unsafe impl<T> Sync for SerdeBincode<T> {}

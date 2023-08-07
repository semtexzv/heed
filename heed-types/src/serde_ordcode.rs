use std::borrow::Cow;
use std::marker::PhantomData;
use ordcode::Order;
use serde::de::DeserializeOwned;
use serde::Serialize;
use heed_traits::{BytesDecode, BytesEncode};

pub struct Ordcode<T>(PhantomData<T>);

impl<'ser, T: Serialize + 'ser> BytesEncode<'ser> for Ordcode<T> {
    type EItem = T;

    fn bytes_encode(item: &'ser Self::EItem) -> Option<Cow<'ser, [u8]>> {
        let key: Cow<'ser, [u8]> = ordcode::ser_to_vec_ordered(item, Order::Ascending)
            .map(Cow::Owned)
            .ok()?;

        if key.len() >= 511 {
            panic!("Key too long");
        }

        Some(key)
    }
}

impl<T: DeserializeOwned + 'static> BytesDecode for Ordcode<T> {
    type DItem = T;

    fn bytes_decode(bytes: &[u8]) -> Option<Self::DItem> {
        ordcode::de_from_bytes_asc(bytes).ok()
    }
}

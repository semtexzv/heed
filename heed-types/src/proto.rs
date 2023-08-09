use std::borrow::Cow;
use std::marker::PhantomData;

use heed_traits::{BytesDecode, BytesEncode};
use protokit::BinProto;

pub struct Proto<T>(PhantomData<T>);

impl<'a, T: BinProto<'a> + 'a> BytesEncode<'a> for Proto<T> {
    type EItem = T;

    fn bytes_encode(item: &'a Self::EItem) -> Option<Cow<'a, [u8]>> {
        protokit::binformat::encode(item).map(Cow::Owned).ok()
    }
}

impl<T: for<'a> BinProto<'a> + 'static + Default> BytesDecode for Proto<T> {
    type DItem = T;

    fn bytes_decode(bytes: &[u8]) -> Option<Self::DItem> {
        protokit::binformat::decode(bytes).ok()
    }
}

use std::borrow::Cow;

pub trait BytesEncode<'a> {
    type EItem: ?Sized + 'a;

    fn bytes_encode(item: &'a Self::EItem) -> Option<Cow<'a, [u8]>>;
}

pub trait BytesDecode {
    type DItem: 'static;

    fn bytes_decode(bytes: &[u8]) -> Option<Self::DItem>;
}

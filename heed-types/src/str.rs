use std::borrow::Cow;

use heed_traits::{BytesDecode, BytesEncode};

use crate::UnalignedSlice;

/// Describes an [`str`].
pub struct Str;

impl BytesEncode<'_> for Str {
    type EItem = str;

    fn bytes_encode(item: &Self::EItem) -> Option<Cow<[u8]>> {
        UnalignedSlice::<u8>::bytes_encode(item.as_bytes())
    }
}

impl BytesDecode for Str {
    type DItem = String;

    fn bytes_decode(bytes: &[u8]) -> Option<Self::DItem> {
        std::str::from_utf8(bytes).ok().map(|v| v.to_string())
    }
}

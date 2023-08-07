pub mod mdb;
pub mod rck;

use std::error::Error;
use std::marker;
use std::ops::{Deref, RangeBounds};

use heed_traits::{BytesDecode, BytesEncode};

type ErrorOf<S> = <S as Store>::Error;
type RtxOf<'e, S> = <S as Store>::Rtx<'e>;
type WtxOf<'e, S> = <S as Store>::Wtx<'e>;

pub trait Store: Sized + Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    type Rtx<'e>: Transaction<Self>
        where
            Self: 'e;

    type Wtx<'e>: Transaction<Self> + Deref<Target=Self::Rtx<'e>>
        where
            Self: 'e;

    type Table<'store>: Table<'store, Store=Self> + Send + Sync
        where
            Self: 'store;

    type Config;

    fn table(&self, name: &str, cfg: &Self::Config) -> Result<Self::Table<'_>, Self::Error>;
    fn typed<KC, DC>(&self, name: &str, cfg: &Self::Config) -> Result<Typed<Self, KC, DC>, Self::Error> {
        Ok(Typed { dyndb: self.table(name, cfg)?, marker: Default::default() })
    }
    fn rtx(&self) -> Result<Self::Rtx<'_>, Self::Error>;
    fn wtx(&self) -> Result<Self::Wtx<'_>, Self::Error>;
    fn with_rtx<R>(
        &self,
        fun: impl FnOnce(&Self, &RtxOf<Self>) -> Result<R, Self::Error>,
    ) -> Result<R, Self::Error> {
        let rtx = self.rtx()?;
        let out = fun(self, &rtx)?;
        rtx.commit()?;

        Ok(out)
    }
    fn with_wtx<R>(
        &self,
        fun: impl FnOnce(&Self, &mut WtxOf<Self>) -> Result<R, Self::Error>,
    ) -> Result<R, Self::Error> {
        let mut rtx = self.wtx()?;
        let out = fun(self, &mut rtx)?;
        rtx.commit()?;

        Ok(out)
    }
}

pub trait Transaction<S: Store>: Sized {
    fn commit(self) -> Result<(), ErrorOf<S>>;
}

pub trait Table<'store>: 'store {
    type Store: Store<Table<'store>=Self>
        where
            Self: 'store;

    type Range<'e, KC: BytesDecode, DC: BytesDecode>: Iterator<
        Item=Result<(KC::DItem, DC::DItem), ErrorOf<Self::Store>>,
    >;

    type RevRange<'e, KC: BytesDecode, DC: BytesDecode>: Iterator<
        Item=Result<(KC::DItem, DC::DItem), ErrorOf<Self::Store>>,
    >;

    fn get<'a, 'txn, KC, DC>(
        &self,
        txn: &'txn RtxOf<Self::Store>,
        key: &'a KC::EItem,
    ) -> Result<Option<DC::DItem>, ErrorOf<Self::Store>>
        where
            KC: BytesEncode<'a>,
            DC: BytesDecode;

    fn range<'a, 'txn, KC, DC, R>(
        &self,
        txn: &'txn RtxOf<Self::Store>,
        range: &'a R,
    ) -> Result<Self::Range<'txn, KC, DC>, ErrorOf<Self::Store>>
        where
            KC: BytesEncode<'a> + BytesDecode,
            DC: BytesDecode,
            R: RangeBounds<KC::EItem>;

    fn rev_range<'a, 'txn, KC, DC, R>(
        &self,
        txn: &'txn RtxOf<Self::Store>,
        range: &'a R,
    ) -> Result<Self::RevRange<'txn, KC, DC>, ErrorOf<Self::Store>>
        where
            KC: BytesEncode<'a> + BytesDecode,
            DC: BytesDecode,
            R: RangeBounds<KC::EItem>;

    fn len<'txn>(&self, txn: &'txn RtxOf<Self::Store>) -> Result<usize, ErrorOf<Self::Store>>;

    fn put<'a, KC, DC>(
        &self,
        txn: &mut WtxOf<Self::Store>,
        key: &'a KC::EItem,
        data: &'a DC::EItem,
    ) -> Result<(), ErrorOf<Self::Store>>
        where
            KC: BytesEncode<'a>,
            DC: BytesEncode<'a>;

    fn delete<'a, KC>(
        &self,
        txn: &mut WtxOf<Self::Store>,
        key: &'a KC::EItem,
    ) -> Result<(), ErrorOf<Self::Store>>
        where
            KC: BytesEncode<'a>;
}

pub struct Typed<'s, S: Store + 's, KC, DC> {
    dyndb: S::Table<'s>,
    marker: marker::PhantomData<(KC, DC)>,
}

impl<'s, S: Store, KC, DC> Clone for Typed<'s, S, KC, DC>
    where
        S::Table<'s>: Clone,
{
    fn clone(&self) -> Self {
        Self { dyndb: self.dyndb.clone(), marker: Default::default() }
    }
}

impl<'s, S: Store, KC, DC> Typed<'s, S, KC, DC> {
    pub fn get<'a, 'txn>(
        &self,
        txn: &'txn RtxOf<S>,
        key: &'a KC::EItem,
    ) -> Result<Option<DC::DItem>, ErrorOf<S>>
        where
            KC: BytesEncode<'a>,
            DC: BytesDecode,
    {
        self.dyndb.get::<KC, DC>(txn, key)
    }

    pub fn range<'a, 'txn, R>(
        &self,
        txn: &'txn RtxOf<S>,
        range: &'a R,
    ) -> Result<<S::Table<'s> as Table<'s>>::Range<'txn, KC, DC>, ErrorOf<S>>
        where
            KC: BytesEncode<'a> + BytesDecode,
            DC: BytesDecode,
            R: RangeBounds<KC::EItem>,
    {
        self.dyndb.range::<KC, DC, R>(txn, range)
    }

    pub fn rev_range<'a, 'txn, R>(
        &self,
        txn: &'txn RtxOf<S>,
        range: &'a R,
    ) -> Result<<S::Table<'s> as Table<'s>>::RevRange<'txn, KC, DC>, ErrorOf<S>>
        where
            KC: BytesEncode<'a> + BytesDecode,
            DC: BytesDecode,
            R: RangeBounds<KC::EItem>,
    {
        self.dyndb.rev_range::<KC, DC, R>(txn, range)
    }

    pub fn len<'txn, T>(&self, txn: &'txn RtxOf<S>) -> Result<usize, ErrorOf<S>> {
        self.dyndb.len(txn)
    }

    pub fn put<'a>(
        &self,
        txn: &mut WtxOf<S>,
        key: &'a KC::EItem,
        data: &'a DC::EItem,
    ) -> Result<(), ErrorOf<S>>
        where
            KC: BytesEncode<'a>,
            DC: BytesEncode<'a>,
    {
        self.dyndb.put::<KC, DC>(txn, key, data)
    }

    pub fn delete<'a>(&self, txn: &mut WtxOf<S>, key: &'a KC::EItem) -> Result<(), ErrorOf<S>>
        where
            KC: BytesEncode<'a>,
    {
        self.dyndb.delete::<KC>(txn, key).map(|_| ())
    }

    pub fn remap_types<KC2, DC2>(self) -> Typed<'s, S, KC2, DC2> {
        Typed { dyndb: self.dyndb, marker: Default::default() }
    }

    /// Change the key codec type of this uniform database, specifying the new codec.
    pub fn remap_key_type<KC2>(self) -> Typed<'s, S, KC2, DC> {
        self.remap_types::<KC2, DC>()
    }

    /// Change the data codec type of this uniform database, specifying the new codec.
    pub fn remap_data_type<DC2>(self) -> Typed<'s, S, KC, DC2> {
        self.remap_types::<KC, DC2>()
    }

    // /// Wrap the data bytes into a lazy decoder.
    // pub fn lazily_decode_data(self) -> Typed<S, KC, LazyDecode<DC>> {
    //     self.remap_types::<KC, LazyDecode<DC>>()
    // }
}

pub struct Tables<S: Store, T> {
    pub store: &'static S,
    pub table: Option<T>,
}

impl<S: Store, T> Tables<S, T> {
    pub fn new<F>(store: S, cfg: &S::Config, make: F) -> Result<Tables<S, T>, S::Error>
        where F: FnOnce(&'static S, &S::Config) -> Result<T, S::Error>
    {
        let store = Box::new(store);
        let store = Box::leak::<'static>(store) as &'static S;
        let o = make(&store, cfg)?;

        Ok(Tables {
            store,
            table: Some(o),
        })
    }
}

impl<S: Store, T> Deref for Tables<S, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.table.as_ref().unwrap_unchecked() }
    }
}

impl<S: Store, T> Drop for Tables<S, T> {
    fn drop(&mut self) {
        drop(self.table.take());
        unsafe {
            drop(Box::from_raw(self.store as *const S as *mut S));
        }
    }
}

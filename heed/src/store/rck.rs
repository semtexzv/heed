use std::borrow::Cow;
use std::collections::Bound;
use std::marker::PhantomData;
use std::ops::{Deref, RangeBounds};
use std::sync::Arc;

use heed_traits::{BytesDecode, BytesEncode};
use rocksdb::{
    BoundColumnFamily, DBIteratorWithThreadMode, Direction, ErrorKind, IteratorMode, MultiThreaded,
    Options, ReadOptions, TransactionDB,
};

use crate::store::{ErrorOf, RtxOf, Store, Table, Transaction, WtxOf};

impl Store for TransactionDB<MultiThreaded> {
    type Error = rocksdb::Error;
    type Rtx<'e> = RockTxn<'e>;
    type Wtx<'e> = RockTxn<'e>;
    type Config = Options;
    type Table<'store> = RockTable<'store>;

    fn table(&self, name: &str, opts: &Self::Config) -> Result<Self::Table<'_>, Self::Error> {
        match self.create_cf(name, opts) {
            Ok(..) => {}
            Err(e)
                if e.kind() == ErrorKind::InvalidArgument
                    && e.to_string().contains("Column family already exists") => {}
            Err(e) => return Err(e),
        };
        let cf = self.cf_handle(name).unwrap();
        Ok(RockTable { cf })
    }

    fn rtx(&self) -> Result<Self::Rtx<'_>, Self::Error> {
        Ok(RockTxn { tx: self.transaction() })
    }

    fn wtx(&self) -> Result<Self::Wtx<'_>, Self::Error> {
        Ok(RockTxn { tx: self.transaction() })
    }
}

pub struct RockTxn<'a> {
    tx: rocksdb::Transaction<'a, TransactionDB<MultiThreaded>>,
}

impl Deref for RockTxn<'_> {
    type Target = Self;

    fn deref(&self) -> &Self::Target {
        self
    }
}

impl Transaction<TransactionDB<MultiThreaded>> for RockTxn<'_> {
    fn commit(self) -> Result<(), ErrorOf<TransactionDB<MultiThreaded>>> {
        rocksdb::Transaction::commit(self.tx)
    }
}

pub struct RockTable<'store> {
    cf: Arc<BoundColumnFamily<'store>>,
}

unsafe impl<'store> Send for RockTable<'store> {}
unsafe impl<'store> Sync for RockTable<'store> {}

pub struct Iter<'a, KC: BytesDecode, DC: BytesDecode> {
    it: DBIteratorWithThreadMode<'a, rocksdb::Transaction<'a, TransactionDB<MultiThreaded>>>,
    _p: PhantomData<(KC, DC)>,
}

impl<'a, KC: BytesDecode, DC: BytesDecode> Iterator for Iter<'a, KC, DC> {
    type Item = Result<(KC::DItem, DC::DItem), rocksdb::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.it.next()? {
            Ok(v) => {
                return Some(Ok((
                    KC::bytes_decode(&v.0).unwrap(),
                    DC::bytes_decode(&v.1).unwrap(),
                )));
            }
            Err(e) => {
                return Some(Err(e));
            }
        }
    }
}

impl<'store> Table<'store> for RockTable<'store> {
    type Store = TransactionDB<MultiThreaded>;
    type Range<'e, KC: BytesDecode, DC: BytesDecode> = Iter<'e, KC, DC>;
    type RevRange<'e, KC: BytesDecode, DC: BytesDecode> = Iter<'e, KC, DC>;

    fn get<'a, 'txn, KC, DC>(
        &self,
        txn: &'txn RtxOf<Self::Store>,
        key: &'a KC::EItem,
    ) -> Result<Option<DC::DItem>, ErrorOf<Self::Store>>
    where
        KC: BytesEncode<'a>,
        DC: BytesDecode,
    {
        let key = KC::bytes_encode(key).unwrap();
        let data = txn.tx.get_pinned_cf_opt(&self.cf, key, &ReadOptions::default())?;

        Ok(data.and_then(|v| {
            let out = DC::bytes_decode(&v);
            out
        }))
    }

    fn range<'a, 'txn, KC, DC, R>(
        &self,
        txn: &'txn RtxOf<Self::Store>,
        range: &'a R,
    ) -> Result<Self::Range<'txn, KC, DC>, ErrorOf<Self::Store>>
    where
        KC: BytesEncode<'a> + BytesDecode,
        DC: BytesDecode,
        R: RangeBounds<KC::EItem>,
    {
        let start = match range.start_bound() {
            Bound::Included(i) => KC::bytes_encode(i),
            Bound::Excluded(i) => KC::bytes_encode(i),
            Bound::Unbounded => Some(Cow::Borrowed(&[][..])),
        }
        .unwrap_or_default();

        let end = match range.end_bound() {
            Bound::Included(i) => {
                KC::bytes_encode(i).map(|v| {
                    let mut v = v.to_vec();
                    crate::iter::advance_key(&mut v);
                    Cow::Owned(v)
                })
            },
            Bound::Excluded(i) => KC::bytes_encode(i),
            Bound::Unbounded => Some(Cow::Borrowed(&[][..])),
        }
            .unwrap_or_default();

        let mut opt = ReadOptions::default();
        opt.set_iterate_upper_bound(end);

        Ok(Iter {
            it: txn.tx.iterator_cf_opt(&self.cf, opt, IteratorMode::From(&start, Direction::Forward)),

            _p: Default::default(),
        })
    }

    fn rev_range<'a, 'txn, KC, DC, R>(
        &self,
        txn: &'txn RtxOf<Self::Store>,
        range: &'a R,
    ) -> Result<Self::RevRange<'txn, KC, DC>, ErrorOf<Self::Store>>
    where
        KC: BytesEncode<'a> + BytesDecode,
        DC: BytesDecode,
        R: RangeBounds<KC::EItem>,
    {


        let start = match range.start_bound() {
            Bound::Included(i) => KC::bytes_encode(i),
            Bound::Excluded(_) => panic!("Excluded lower bound"),
            Bound::Unbounded => Some(Cow::Borrowed(&[][..])),
        }
            .unwrap_or_default();

        let end = match range.end_bound() {
            Bound::Included(i) => KC::bytes_encode(i),
            Bound::Excluded(i) => KC::bytes_encode(i),
            Bound::Unbounded => Some(Cow::Borrowed(&[][..])),
        }
            .unwrap_or_default();

        let mut opt = ReadOptions::default();
        opt.set_iterate_lower_bound(start);

        Ok(Iter {
            it: txn.tx.iterator_cf(&self.cf, IteratorMode::From(&end, Direction::Reverse)),
            _p: Default::default(),
        })
    }

    fn len<'txn>(&self, txn: &'txn RtxOf<Self::Store>) -> Result<usize, ErrorOf<Self::Store>> {
        Ok(txn.tx.iterator(IteratorMode::Start).count())
    }

    fn put<'a, KC, DC>(
        &self,
        txn: &mut WtxOf<Self::Store>,
        key: &'a KC::EItem,
        data: &'a DC::EItem,
    ) -> Result<(), ErrorOf<Self::Store>>
    where
        KC: BytesEncode<'a>,
        DC: BytesEncode<'a>,
    {
        let k = KC::bytes_encode(key).unwrap();
        let v = DC::bytes_encode(data).unwrap();
        txn.tx.put_cf(&self.cf, k, v)?;

        Ok(())
    }

    fn delete<'a, KC>(
        &self,
        txn: &mut WtxOf<Self::Store>,
        key: &'a KC::EItem,
    ) -> Result<(), ErrorOf<Self::Store>>
    where
        KC: BytesEncode<'a>,
    {
        let k = KC::bytes_encode(key).unwrap();
        txn.tx.delete_cf(&self.cf, k)?;
        Ok(())
    }
}

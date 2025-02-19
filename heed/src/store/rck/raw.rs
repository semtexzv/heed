use std::collections::Bound;
use std::marker::PhantomData;
use std::ops::{Deref, RangeBounds};
use std::sync::Arc;

use heed_traits::{BytesDecode, BytesEncode};
use rocksdb::{
    BoundColumnFamily, DBIteratorWithThreadMode, DBWithThreadMode, Direction, ErrorKind,
    IteratorMode, MultiThreaded, Options, ReadOptions,
};

use crate::iter::advance_key;
use crate::store::{ErrorOf, RtxOf, Store, Table, Transaction, WtxOf};

pub type DBType = DBWithThreadMode<MultiThreaded>;

impl Store for DBType {
    type Error = rocksdb::Error;
    type Rtx<'e> = RawTxn<'e>;
    type Wtx<'e> = WRawTxn<'e>;
    type Table<'store> = RockTable<'store>;
    type Config = Options;

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
        Ok(RawTxn { db: self })
    }

    fn wtx(&self) -> Result<Self::Wtx<'_>, Self::Error> {
        Ok(WRawTxn { rtx: RawTxn { db: self } })
    }
}

pub struct WRawTxn<'a> {
    rtx: RawTxn<'a>,
}

impl<'a> Deref for WRawTxn<'a> {
    type Target = RawTxn<'a>;

    fn deref(&self) -> &Self::Target {
        &self.rtx
    }
}

impl Transaction<DBType> for WRawTxn<'_> {
    fn commit(self) -> Result<(), ErrorOf<DBType>> {
        Ok(())
    }
}

pub struct RawTxn<'a> {
    db: &'a DBType,
}

impl Transaction<DBType> for RawTxn<'_> {
    fn commit(self) -> Result<(), ErrorOf<DBType>> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct RockTable<'store> {
    cf: Arc<BoundColumnFamily<'store>>,
}

unsafe impl<'store> Send for RockTable<'store> {}

unsafe impl<'store> Sync for RockTable<'store> {}

pub struct Iter<'a, KC: BytesDecode, DC: BytesDecode> {
    it: DBIteratorWithThreadMode<'a, DBType>,
    _p: PhantomData<(KC, DC)>,
}

impl<'a, KC: BytesDecode, DC: BytesDecode> Iterator for Iter<'a, KC, DC> {
    type Item = Result<(KC::DItem, DC::DItem), rocksdb::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.it.next()? {
            Ok(v) => {
                // println!("iter {:?} => {:?}", &v.0, &v.1);
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
    type Store = DBType;
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
        let data = txn.db.get_pinned_cf_opt(&self.cf, key, &ReadOptions::default())?;

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
        let mut opt = ReadOptions::default();

        match range.end_bound() {
            Bound::Included(i) => {
                let mut v = KC::bytes_encode(i).unwrap().to_vec();
                crate::iter::advance_key(&mut v);
                opt.set_iterate_upper_bound(v);
            }
            Bound::Excluded(i) => {
                opt.set_iterate_upper_bound(KC::bytes_encode(i).unwrap());
            }
            _ => {}
        };

        let it = match range.start_bound() {
            Bound::Included(i) => {
                let k = KC::bytes_encode(i).unwrap().to_vec();
                txn.db.iterator_cf_opt(&self.cf, opt, IteratorMode::From(&k, Direction::Forward))
            }
            Bound::Excluded(i) => {
                let mut k = KC::bytes_encode(i).unwrap().to_vec();
                advance_key(&mut k);

                txn.db.iterator_cf_opt(&self.cf, opt, IteratorMode::From(&k, Direction::Forward))
            }
            Bound::Unbounded => txn.db.iterator_cf_opt(&self.cf, opt, IteratorMode::Start),
        };

        Ok(Iter { it, _p: Default::default() })
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
        let mut opt = ReadOptions::default();

        match range.start_bound() {
            Bound::Included(i) => {
                let v = KC::bytes_encode(i).unwrap().to_vec();
                opt.set_iterate_lower_bound(v);
            }
            Bound::Excluded(..) => {
                panic!("Excluded lower bound");
            }
            _ => {}
        };

        let it = match range.end_bound() {
            Bound::Included(i) => {
                let k = KC::bytes_encode(i).unwrap();
                txn.db.iterator_cf_opt(&self.cf, opt, IteratorMode::From(&k, Direction::Reverse))
            }
            Bound::Excluded(i) => {
                let mut k = KC::bytes_encode(i).unwrap().to_vec();
                crate::iter::retreat_key(&mut k);
                txn.db.iterator_cf_opt(&self.cf, opt, IteratorMode::From(&k, Direction::Reverse))
            }
            Bound::Unbounded => txn.db.iterator_cf_opt(&self.cf, opt, IteratorMode::End),
        };

        Ok(Iter { it, _p: Default::default() })
    }

    fn len<'txn>(&self, txn: &'txn RtxOf<Self::Store>) -> Result<usize, ErrorOf<Self::Store>> {
        Ok(txn.db.iterator(IteratorMode::Start).count())
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
        txn.rtx.db.put_cf(&self.cf, k, v)?;

        Ok(())
    }

    fn append<'a, KC, DC>(
        &self,
        txn: &mut WtxOf<Self::Store>,
        key: &'a KC::EItem,
        data: &'a DC::EItem,
    ) -> Result<(), ErrorOf<Self::Store>>
    where
        KC: BytesEncode<'a>,
        DC: BytesEncode<'a>,
    {
        self.put::<KC, DC>(txn, key, data)
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
        txn.rtx.db.delete_cf(&self.cf, k)?;
        Ok(())
    }

    fn clear(&self, txn: &mut WtxOf<Self::Store>) -> Result<(), ErrorOf<Self::Store>> {
        txn.rtx.db.delete_range_cf(&self.cf, &[][..], &vec![0xFF; 512][..])?;

        Ok(())
    }
}

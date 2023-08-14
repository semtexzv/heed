use std::ops::RangeBounds;

use heed_traits::{BytesDecode, BytesEncode};

use crate::store::{ErrorOf, RtxOf, Store, Table, Transaction, WtxOf};
use crate::{Env, PolyDatabase, RoRange, RoRevRange, RoTxn, RwTxn};

impl Store for Env {
    type Error = crate::Error;
    type Rtx<'e> = RoTxn<'e>;
    type Wtx<'e> = RwTxn<'e, 'e>;
    type Table<'store> = PolyDatabase;
    type Config = ();

    fn table(&self, name: &str, _cfg: &Self::Config) -> Result<Self::Table<'_>, Self::Error> {
        let mut wtx = self.wtx()?;
        let db = self.create_poly_database(&mut wtx, Some(name))?;
        wtx.commit()?;

        Ok(db)
    }

    fn rtx(&self) -> Result<Self::Rtx<'_>, Self::Error> {
        self.read_txn()
    }

    fn wtx(&self) -> Result<Self::Wtx<'_>, Self::Error> {
        self.write_txn()
    }
}

impl Transaction<Env> for RoTxn<'_> {
    fn commit(self) -> Result<(), ErrorOf<Env>> {
        RoTxn::commit(self)
    }
}

impl Transaction<Env> for RwTxn<'_, '_> {
    fn commit(self) -> Result<(), ErrorOf<Env>> {
        RwTxn::commit(self)
    }
}

impl<'store> Table<'store> for PolyDatabase {
    type Store = Env;
    type Range<'e, KC: BytesDecode, DC: BytesDecode> = RoRange<'e, KC, DC>;
    type RevRange<'e, KC: BytesDecode, DC: BytesDecode> = RoRevRange<'e, KC, DC>;

    fn get<'a, 'txn, KC, DC>(
        &self,
        txn: &'txn RtxOf<Self::Store>,
        key: &'a KC::EItem,
    ) -> Result<Option<DC::DItem>, ErrorOf<Self::Store>>
    where
        KC: BytesEncode<'a>,
        DC: BytesDecode,
    {
        PolyDatabase::get::<(), KC, DC>(self, txn, key)
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
        PolyDatabase::range(self, txn, range)
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
        PolyDatabase::rev_range(self, txn, range)
    }

    fn len<'txn>(&self, txn: &'txn RtxOf<Self::Store>) -> Result<usize, ErrorOf<Self::Store>> {
        PolyDatabase::len(self, txn)
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
        PolyDatabase::put::<(), KC, DC>(self, txn, key, data)
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
        PolyDatabase::append::<(), KC, DC>(self, txn, key, data)
    }

    fn delete<'a, KC>(
        &self,
        txn: &mut WtxOf<Self::Store>,
        key: &'a KC::EItem,
    ) -> Result<(), ErrorOf<Self::Store>>
    where
        KC: BytesEncode<'a>,
    {
        PolyDatabase::delete::<(), KC>(self, txn, key).map(|_| ())
    }

    fn clear(&self, txn: &mut WtxOf<Self::Store>) -> Result<(), ErrorOf<Self::Store>> {
        PolyDatabase::clear(self, txn)
    }
}

use crate::tree_store::{
    AccessGuardMut, Btree, BtreeDrain, BtreeMut, BtreeRangeIter, Checksum, PageHint, PageNumber,
    TransactionalMemory,
};
use crate::types::{RedbKey, RedbValue};
use crate::Result;
use crate::{AccessGuard, WriteTransaction};
use std::borrow::Borrow;
use std::cell::RefCell;
use std::ops::RangeBounds;
use std::rc::Rc;

/// A table containing key-value mappings
pub struct Table<'db, 'txn, K: RedbKey + ?Sized + 'txn, V: RedbValue + ?Sized + 'txn> {
    name: String,
    transaction: &'txn WriteTransaction<'db>,
    tree: BtreeMut<'txn, K, V>,
}

impl<'db, 'txn, K: RedbKey + ?Sized + 'txn, V: RedbValue + ?Sized + 'txn> Table<'db, 'txn, K, V> {
    pub(crate) fn new(
        name: &str,
        table_root: Option<(PageNumber, Checksum)>,
        freed_pages: Rc<RefCell<Vec<PageNumber>>>,
        mem: &'db TransactionalMemory,
        transaction: &'txn WriteTransaction<'db>,
    ) -> Table<'db, 'txn, K, V> {
        Table {
            name: name.to_string(),
            transaction,
            tree: BtreeMut::new(table_root, mem, freed_pages),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn print_debug(&self, include_values: bool) -> Result {
        self.tree.print_debug(include_values)
    }

    /// Removes and returns the first key-value pair in the table
    pub fn pop_first(&mut self) -> Result<Option<(AccessGuard<K>, AccessGuard<V>)>> {
        // TODO: optimize this
        let first = self.iter()?.next();
        if let Some((ref key, _)) = first {
            let owned_key = K::as_bytes(key.value().borrow()).as_ref().to_vec();
            drop(first);
            let key = K::from_bytes(&owned_key);
            let value = self.remove(&key)?.unwrap();
            drop(key);
            Ok(Some((AccessGuard::with_owned_value(owned_key), value)))
        } else {
            Ok(None)
        }
    }

    /// Removes and returns the last key-value pair in the table
    pub fn pop_last(&mut self) -> Result<Option<(AccessGuard<K>, AccessGuard<V>)>> {
        // TODO: optimize this
        let first = self.iter()?.rev().next();
        if let Some((ref key, _)) = first {
            let owned_key = K::as_bytes(key.value().borrow()).as_ref().to_vec();
            drop(first);
            let key = K::from_bytes(&owned_key);
            let value = self.remove(&key)?.unwrap();
            drop(key);
            Ok(Some((AccessGuard::with_owned_value(owned_key), value)))
        } else {
            Ok(None)
        }
    }

    pub fn drain<'a, KR>(
        &'a mut self,
        range: impl RangeBounds<KR> + Clone + 'a,
    ) -> Result<Drain<'a, K, V>>
    where
        K: 'a,
        // TODO: we should not require Clone here
        KR: Borrow<K::SelfType<'a>> + ?Sized + Clone + 'a,
    {
        // Safety: No other references to this table can exist.
        // Tables can only be opened mutably in one location (see Error::TableAlreadyOpen),
        // and we borrow &mut self.
        unsafe { self.tree.drain(range).map(Drain::new) }
    }

    /// Insert mapping of the given key to the given value
    ///
    /// Returns the old value, if the key was present in the table
    pub fn insert<'a>(
        &mut self,
        key: impl Borrow<K::SelfType<'a>>,
        value: impl Borrow<V::SelfType<'a>>,
    ) -> Result<Option<AccessGuard<V>>>
    where
        K: 'a,
        V: 'a,
    {
        // Safety: No other references to this table can exist.
        // Tables can only be opened mutably in one location (see Error::TableAlreadyOpen),
        // and we borrow &mut self.
        unsafe { self.tree.insert(key.borrow(), value.borrow()) }
    }

    /// Reserve space to insert a key-value pair
    /// The returned reference will have length equal to value_length
    // TODO: return type should be V, not [u8]
    pub fn insert_reserve<'a>(
        &mut self,
        key: impl Borrow<K::SelfType<'a>>,
        value_length: usize,
    ) -> Result<AccessGuardMut<K, &[u8]>>
    where
        K: 'a,
    {
        // Safety: No other references to this table can exist.
        // Tables can only be opened mutably in one location (see Error::TableAlreadyOpen),
        // and we borrow &mut self.
        unsafe { self.tree.insert_reserve(key.borrow(), value_length) }
    }

    /// Removes the given key
    ///
    /// Returns the old value, if the key was present in the table
    pub fn remove<'a>(
        &mut self,
        key: impl Borrow<K::SelfType<'a>>,
    ) -> Result<Option<AccessGuard<V>>>
    where
        K: 'a,
    {
        // Safety: No other references to this table can exist.
        // Tables can only be opened mutably in one location (see Error::TableAlreadyOpen),
        // and we borrow &mut self.
        unsafe { self.tree.remove(key.borrow()) }
    }
}

impl<'db, 'txn, K: RedbKey + ?Sized, V: RedbValue + ?Sized> ReadableTable<K, V>
    for Table<'db, 'txn, K, V>
{
    fn get<'a>(&self, key: impl Borrow<K::SelfType<'a>>) -> Result<Option<AccessGuard<V>>>
    where
        K: 'a,
    {
        self.tree.get(key.borrow())
    }

    fn range<'a, KR>(&'a self, range: impl RangeBounds<KR> + 'a) -> Result<RangeIter<'a, K, V>>
    where
        K: 'a,
        KR: Borrow<K::SelfType<'a>> + ?Sized + 'a,
    {
        self.tree.range(range).map(RangeIter::new)
    }

    fn len(&self) -> Result<usize> {
        self.tree.len()
    }

    fn is_empty(&self) -> Result<bool> {
        self.len().map(|x| x == 0)
    }
}

impl<'db, 'txn, K: RedbKey + ?Sized, V: RedbValue + ?Sized> Drop for Table<'db, 'txn, K, V> {
    fn drop(&mut self) {
        self.transaction.close_table(&self.name, &mut self.tree);
    }
}

pub trait ReadableTable<K: RedbKey + ?Sized, V: RedbValue + ?Sized> {
    /// Returns the value corresponding to the given key
    fn get<'a>(&self, key: impl Borrow<K::SelfType<'a>>) -> Result<Option<AccessGuard<V>>>
    where
        K: 'a;

    /// Returns a double-ended iterator over a range of elements in the table
    ///
    /// # Examples
    ///
    /// Usage:
    /// ```rust
    /// use redb::*;
    /// # use tempfile::NamedTempFile;
    /// const TABLE: TableDefinition<&str, u64> = TableDefinition::new("my_data");
    ///
    /// # fn main() -> Result<(), Error> {
    /// # let tmpfile: NamedTempFile = NamedTempFile::new().unwrap();
    /// # let filename = tmpfile.path();
    /// let db = unsafe { Database::create(filename)? };
    /// let write_txn = db.begin_write()?;
    /// {
    ///     let mut table = write_txn.open_table(TABLE)?;
    ///     table.insert("a", &0)?;
    ///     table.insert("b", &1)?;
    ///     table.insert("c", &2)?;
    /// }
    /// write_txn.commit()?;
    ///
    /// let read_txn = db.begin_read()?;
    /// let table = read_txn.open_table(TABLE)?;
    /// let mut iter = table.range("a".."c")?;
    /// let (key, value) = iter.next().unwrap();
    /// assert_eq!("a", key.value());
    /// assert_eq!(0, value.value());
    /// # Ok(())
    /// # }
    /// ```
    fn range<'a, KR>(&'a self, range: impl RangeBounds<KR> + 'a) -> Result<RangeIter<'a, K, V>>
    where
        K: 'a,
        KR: Borrow<K::SelfType<'a>> + ?Sized + 'a;

    /// Returns the number of entries in the table
    fn len(&self) -> Result<usize>;

    /// Returns `true` if the table is empty
    fn is_empty(&self) -> Result<bool>;

    /// Returns a double-ended iterator over all elements in the table
    fn iter(&self) -> Result<RangeIter<K, V>> {
        self.range::<K::SelfType<'_>>(..)
    }
}

/// A read-only table
pub struct ReadOnlyTable<'txn, K: RedbKey + ?Sized, V: RedbValue + ?Sized> {
    tree: Btree<'txn, K, V>,
}

impl<'txn, K: RedbKey + ?Sized, V: RedbValue + ?Sized> ReadOnlyTable<'txn, K, V> {
    pub(crate) fn new(
        root_page: Option<(PageNumber, Checksum)>,
        hint: PageHint,
        mem: &'txn TransactionalMemory,
    ) -> ReadOnlyTable<'txn, K, V> {
        ReadOnlyTable {
            tree: Btree::new(root_page, hint, mem),
        }
    }
}

impl<'txn, K: RedbKey + ?Sized, V: RedbValue + ?Sized> ReadableTable<K, V>
    for ReadOnlyTable<'txn, K, V>
{
    fn get<'a>(&self, key: impl Borrow<K::SelfType<'a>>) -> Result<Option<AccessGuard<V>>>
    where
        K: 'a,
    {
        self.tree.get(key.borrow())
    }

    fn range<'a, KR>(&'a self, range: impl RangeBounds<KR> + 'a) -> Result<RangeIter<'a, K, V>>
    where
        K: 'a,
        KR: Borrow<K::SelfType<'a>> + ?Sized + 'a,
    {
        self.tree.range(range).map(RangeIter::new)
    }

    fn len(&self) -> Result<usize> {
        self.tree.len()
    }

    fn is_empty(&self) -> Result<bool> {
        self.len().map(|x| x == 0)
    }
}

pub struct Drain<'a, K: RedbKey + ?Sized + 'a, V: RedbValue + ?Sized + 'a> {
    inner: BtreeDrain<'a, K, V>,
}

impl<'a, K: RedbKey + ?Sized + 'a, V: RedbValue + ?Sized + 'a> Drain<'a, K, V> {
    fn new(inner: BtreeDrain<'a, K, V>) -> Self {
        Self { inner }
    }
}

impl<'a, K: RedbKey + ?Sized + 'a, V: RedbValue + ?Sized + 'a> Iterator for Drain<'a, K, V> {
    type Item = (AccessGuard<'a, K>, AccessGuard<'a, V>);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.inner.next()?;
        let (page, key_range, value_range) = entry.into_raw();
        let key = AccessGuard::with_page(page.clone(), key_range);
        let value = AccessGuard::with_page(page, value_range);
        Some((key, value))
    }
}

impl<'a, K: RedbKey + ?Sized + 'a, V: RedbValue + ?Sized + 'a> DoubleEndedIterator
    for Drain<'a, K, V>
{
    fn next_back(&mut self) -> Option<Self::Item> {
        let entry = self.inner.next_back()?;
        let (page, key_range, value_range) = entry.into_raw();
        let key = AccessGuard::with_page(page.clone(), key_range);
        let value = AccessGuard::with_page(page, value_range);
        Some((key, value))
    }
}

pub struct RangeIter<'a, K: RedbKey + ?Sized + 'a, V: RedbValue + ?Sized + 'a> {
    inner: BtreeRangeIter<'a, K, V>,
}

impl<'a, K: RedbKey + ?Sized + 'a, V: RedbValue + ?Sized + 'a> RangeIter<'a, K, V> {
    fn new(inner: BtreeRangeIter<'a, K, V>) -> Self {
        Self { inner }
    }
}

impl<'a, K: RedbKey + ?Sized + 'a, V: RedbValue + ?Sized + 'a> Iterator for RangeIter<'a, K, V> {
    type Item = (AccessGuard<'a, K>, AccessGuard<'a, V>);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(entry) = self.inner.next() {
            let (page, key_range, value_range) = entry.into_raw();
            let key = AccessGuard::with_page(page.clone(), key_range);
            let value = AccessGuard::with_page(page, value_range);
            Some((key, value))
        } else {
            None
        }
    }
}

impl<'a, K: RedbKey + ?Sized + 'a, V: RedbValue + ?Sized + 'a> DoubleEndedIterator
    for RangeIter<'a, K, V>
{
    fn next_back(&mut self) -> Option<Self::Item> {
        if let Some(entry) = self.inner.next_back() {
            let (page, key_range, value_range) = entry.into_raw();
            let key = AccessGuard::with_page(page.clone(), key_range);
            let value = AccessGuard::with_page(page, value_range);
            Some((key, value))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod test {
    use crate::types::{RedbKey, RedbValue, Sealed};
    use crate::{Database, ReadableTable, TableDefinition};
    use std::cmp::Ordering;
    use tempfile::NamedTempFile;

    #[test]
    fn custom_ordering() {
        #[derive(Debug)]
        struct ReverseKey(Vec<u8>);

        impl RedbValue for ReverseKey {
            type SelfType<'a> = ReverseKey
            where
                Self: 'a;
            type AsBytes<'a> = &'a [u8]
            where
                Self: 'a;

            fn fixed_width() -> Option<usize> {
                None
            }

            fn from_bytes<'a>(data: &'a [u8]) -> ReverseKey
            where
                Self: 'a,
            {
                ReverseKey(data.to_vec())
            }

            fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> &'a [u8]
            where
                Self: 'a,
                Self: 'b,
            {
                &value.0
            }

            fn redb_type_name() -> String {
                "ReverseKey".to_string()
            }
        }

        impl Sealed for ReverseKey {}

        impl RedbKey for ReverseKey {
            fn compare(data1: &[u8], data2: &[u8]) -> Ordering {
                data2.cmp(data1)
            }
        }

        let definition: TableDefinition<ReverseKey, &str> = TableDefinition::new("x");

        let tmpfile: NamedTempFile = NamedTempFile::new().unwrap();
        let db = Database::create(tmpfile.path()).unwrap();
        let write_txn = db.begin_write().unwrap();
        {
            let mut table = write_txn.open_table(definition).unwrap();
            for i in 0..10u8 {
                let key = vec![i];
                table.insert(&ReverseKey(key), "value").unwrap();
            }
        }
        write_txn.commit().unwrap();

        let read_txn = db.begin_read().unwrap();
        let table = read_txn.open_table(definition).unwrap();
        let start = ReverseKey(vec![7u8]); // ReverseKey is used, so 7 < 3
        let end = ReverseKey(vec![3u8]);
        let mut iter = table.range(start..=end).unwrap();
        for i in (3..=7u8).rev() {
            let (key, value) = iter.next().unwrap();
            assert_eq!(&[i], key.value().0.as_slice());
            assert_eq!("value", value.value());
        }
        assert!(iter.next().is_none());
    }
}

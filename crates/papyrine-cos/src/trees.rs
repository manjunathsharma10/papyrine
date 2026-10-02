//! Generic name trees (string keys, e.g. `/Dests`, `/EmbeddedFiles`) and number trees (integer
//! keys, e.g. `/PageLabels`). The root is any tree node: a dictionary with `/Names`, `/Nums` or
//! `/Kids`. Trees are rebalanced by qpdf on insert.

use qpdf_sys as ffi;

use crate::{Document, Object, Result};

impl Document {
    /// A new, empty name tree root (a direct dictionary; attach it with `dict_set`).
    pub fn new_name_tree(&self) -> Result<Object> {
        Ok(self.wrap(ffi::name_tree_new(self.ffi())?))
    }

    pub fn name_tree_keys(&self, root: &Object) -> Result<Vec<Vec<u8>>> {
        Ok(ffi::name_tree_keys(self.ffi(), root.raw())?
            .into_iter()
            .map(|b| b.v)
            .collect())
    }

    pub fn name_tree_get(&self, root: &Object, key: impl AsRef<[u8]>) -> Result<Option<Object>> {
        let r = ffi::name_tree_get(self.ffi(), root.raw(), key.as_ref())?;
        Ok((!r.is_null()).then(|| self.wrap(r)))
    }

    /// Insert or replace an entry.
    pub fn name_tree_set(
        &self,
        root: &Object,
        key: impl AsRef<[u8]>,
        value: &Object,
    ) -> Result<()> {
        Ok(ffi::name_tree_set(
            self.ffi(),
            root.raw(),
            key.as_ref(),
            value.raw(),
        )?)
    }

    /// Returns whether the key existed.
    pub fn name_tree_remove(&self, root: &Object, key: impl AsRef<[u8]>) -> Result<bool> {
        Ok(ffi::name_tree_remove(self.ffi(), root.raw(), key.as_ref())?)
    }

    pub fn new_number_tree(&self) -> Result<Object> {
        Ok(self.wrap(ffi::number_tree_new(self.ffi())?))
    }

    pub fn number_tree_keys(&self, root: &Object) -> Result<Vec<i64>> {
        Ok(ffi::number_tree_keys(self.ffi(), root.raw())?)
    }

    pub fn number_tree_get(&self, root: &Object, key: i64) -> Result<Option<Object>> {
        let r = ffi::number_tree_get(self.ffi(), root.raw(), key)?;
        Ok((!r.is_null()).then(|| self.wrap(r)))
    }

    pub fn number_tree_set(&self, root: &Object, key: i64, value: &Object) -> Result<()> {
        Ok(ffi::number_tree_set(
            self.ffi(),
            root.raw(),
            key,
            value.raw(),
        )?)
    }

    pub fn number_tree_remove(&self, root: &Object, key: i64) -> Result<bool> {
        Ok(ffi::number_tree_remove(self.ffi(), root.raw(), key)?)
    }

    /// The `/Root /Names /<name>` tree (e.g. `"Dests"`, `"EmbeddedFiles"`), creating the
    /// `/Names` dictionary and an empty tree when `create` is set and it is missing.
    pub fn catalog_name_tree(&self, name: &str, create: bool) -> Result<Option<Object>> {
        let root = self.root()?;
        let mut names = root.dict_get("Names")?;
        if !names.is_dictionary() {
            if !create {
                return Ok(None);
            }
            names = self.new_dict();
            root.dict_set("Names", &names)?;
        }
        let tree = names.dict_get(name)?;
        if tree.is_dictionary() {
            return Ok(Some(tree));
        }
        if !create {
            return Ok(None);
        }
        let tree = self.new_name_tree()?;
        names.dict_set(name, &tree)?;
        Ok(Some(tree))
    }
}

impl Object {
    pub(crate) fn is_dictionary(&self) -> bool {
        matches!(self.kind(), Ok(crate::ObjectKind::Dictionary))
    }
}

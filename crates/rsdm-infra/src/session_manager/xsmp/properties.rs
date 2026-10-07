//! Retain XSMP properties without executing their command values.

use std::{collections::HashMap, ffi::CStr, ptr, slice};

use super::ffi;

const MAX_BYTES: usize = 1_048_576;
const MAX_PROPERTIES: usize = 256;

#[derive(Default)]
pub(super) struct Properties(HashMap<String, OwnedProperty>);

struct OwnedProperty(*mut ffi::Property);

impl Drop for OwnedProperty {
    fn drop(&mut self) {
        // SAFETY: ownership was transferred from the libSM callback.
        if !self.0.is_null() { unsafe { ffi::SmFreeProperty(self.0); } }
    }
}

impl Properties {
    pub unsafe fn set(&mut self, count: i32, properties: *mut *mut ffi::Property) {
        let mut incoming = IncomingProperties { count: count.max(0) as usize, properties };
        for index in 0..incoming.count {
            let property = incoming.take(index);
            let Some(name) = property.name() else { continue; };
            let total = self.0.values().map(OwnedProperty::bytes).sum::<usize>() + property.bytes();
            if total <= MAX_BYTES && (self.0.contains_key(&name) || self.0.len() < MAX_PROPERTIES) {
                self.0.insert(name, property);
            }
        }
    }

    pub unsafe fn delete(&mut self, count: i32, names: *mut *mut libc::c_char) {
        for index in 0..count.max(0) as usize {
            let name = unsafe { *names.add(index) };
            if !name.is_null() {
                let name = unsafe { CStr::from_ptr(name) }.to_string_lossy();
                self.0.remove(name.as_ref());
            }
        }
        // SAFETY: libSM transferred the names array and each string.
        unsafe { ffi::SmFreeReasons(count, names); }
    }

    pub unsafe fn reply(&self, connection: ffi::SmsConn) {
        let mut values: Vec<_> = self.0.values().map(|value| value.0).collect();
        // SAFETY: retained properties remain valid for this synchronous call.
        unsafe { ffi::SmsReturnProperties(connection, values.len() as i32, values.as_mut_ptr()); }
    }
}

impl OwnedProperty {
    fn name(&self) -> Option<String> {
        if self.0.is_null() { return None; }
        // SAFETY: libSM parsed and allocated this property and its strings.
        let property = unsafe { &*self.0 };
        if property.name.is_null() || property.kind.is_null() || property.count < 0 { return None; }
        let name = unsafe { CStr::from_ptr(property.name) };
        (name.to_bytes().len() <= 256).then(|| name.to_string_lossy().into_owned())
    }

    fn bytes(&self) -> usize {
        if self.0.is_null() { return MAX_BYTES + 1; }
        // SAFETY: libSM allocated values according to the parsed count.
        let property = unsafe { &*self.0 };
        if property.name.is_null() || property.kind.is_null() || property.count < 0
            || (property.count > 0 && property.values.is_null())
        { return MAX_BYTES + 1; }
        let values = if property.count == 0 { &[] } else {
            unsafe { slice::from_raw_parts(property.values, property.count as usize) }
        };
        let metadata = unsafe { CStr::from_ptr(property.name).to_bytes().len() + CStr::from_ptr(property.kind).to_bytes().len() }
            .saturating_add(size_of::<ffi::Property>()).saturating_add(size_of_val(values));
        values.iter().fold(metadata, |total, value| total.saturating_add(
            usize::try_from(value.length).unwrap_or(MAX_BYTES + 1)))
    }
}

struct IncomingProperties {
    count: usize,
    properties: *mut *mut ffi::Property,
}

impl IncomingProperties {
    fn take(&mut self, index: usize) -> OwnedProperty {
        // SAFETY: index comes from this array's count; replacing with null
        // transfers ownership and prevents its destructor from freeing twice.
        OwnedProperty(unsafe { ptr::replace(self.properties.add(index), ptr::null_mut()) })
    }
}

impl Drop for IncomingProperties {
    fn drop(&mut self) {
        for index in 0..self.count {
            // SAFETY: untouched entries still belong to this received array.
            let property = unsafe { *self.properties.add(index) };
            if !property.is_null() { unsafe { ffi::SmFreeProperty(property); } }
        }
        unsafe { libc::free(self.properties.cast()); }
    }
}

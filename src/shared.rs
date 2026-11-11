//! Native values derived from a JS class instance.
//!
//! This replaces napi's `SharedReference`, which must not be used for values
//! that are created per call (object lookups, iterators, ...). napi's
//! `Reference::share_with` does not tie the derived value to the
//! `SharedReference` that holds it. Instead it adds the value's destructor to
//! the *owner's* finalize callback chain. Dropping the `SharedReference` only
//! unrefs the owner, so every derived value (e.g. every `git2::Object` returned
//! by `Repository#getObject`) stays alive until the owning `Repository` is
//! garbage collected. The callback chain also grows by one nested closure per
//! call. Running it is recursive, so collecting a `Repository` that has served
//! ~1M lookups overflows the stack.
//!
//! [`Shared`] owns its value directly and frees it when the last clone is
//! dropped. It holds a strong napi reference to the owner, so the owner (and
//! the native data the value borrows from) outlives the value.

use napi::bindgen_prelude::{Env, Reference, Result};
use std::any::Any;
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

/// A value of type `S` that may borrow from the native data of the JS class
/// instance `T` (expressed with a `'static` lifetime on `S`).
pub struct Shared<T: 'static, S: 'static> {
  // Fields drop in declaration order: the value first, then whatever it
  // borrows from (parent values, then the owner).
  value: Rc<UnsafeCell<S>>,
  parent: Option<Rc<dyn Any>>,
  owner: Reference<T>,
}

impl<T: 'static, S: 'static> Shared<T, S> {
  /// Clone the handle. The value is shared, not copied, and is freed once
  /// every clone has been dropped.
  pub fn clone(&self, env: Env) -> Result<Self> {
    Ok(Self {
      value: self.value.clone(),
      parent: self.parent.clone(),
      owner: self.owner.clone(env)?,
    })
  }

  /// Derive a new value from this one. The new value may borrow from this
  /// value, which is kept alive until the new value is dropped.
  pub fn share<U: 'static, F: FnOnce(&'static mut S) -> Result<U>>(self, _env: Env, f: F) -> Result<Shared<T, U>> {
    // SAFETY: the value lives in an `Rc` allocation (stable address), and the
    // `Rc` is stored in the returned `Shared` and dropped after `U`.
    let value = f(unsafe { &mut *self.value.get() })?;
    Ok(Shared {
      value: Rc::new(UnsafeCell::new(value)),
      parent: Some(Rc::new((self.value, self.parent))),
      owner: self.owner,
    })
  }
}

impl<T: 'static, S: 'static> Deref for Shared<T, S> {
  type Target = S;

  fn deref(&self) -> &Self::Target {
    unsafe { &*self.value.get() }
  }
}

impl<T: 'static, S: 'static> DerefMut for Shared<T, S> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    unsafe { &mut *self.value.get() }
  }
}

pub trait ShareExt<T: 'static> {
  /// Derive a value from a class instance. Unlike napi's `share_with`, the
  /// value is freed when the returned [`Shared`] is dropped.
  fn share<S: 'static, F: FnOnce(&'static mut T) -> Result<S>>(self, env: Env, f: F) -> Result<Shared<T, S>>;
}

impl<T: 'static> ShareExt<T> for Reference<T> {
  fn share<S: 'static, F: FnOnce(&'static mut T) -> Result<S>>(mut self, _env: Env, f: F) -> Result<Shared<T, S>> {
    // SAFETY: `owner` keeps the class instance (and so `T`) alive for as long
    // as the returned `Shared`, which drops the derived value first.
    let owner: &'static mut T = unsafe { &mut *(self.deref_mut() as *mut T) };
    let value = f(owner)?;
    Ok(Shared {
      value: Rc::new(UnsafeCell::new(value)),
      parent: None,
      owner: self,
    })
  }
}

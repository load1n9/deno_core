// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.
use deno_proc_macro_rules::rules;
use proc_macro2::Ident;
use proc_macro2::Span;
use proc_macro2::TokenStream;
use quote::format_ident;
use quote::quote;
use quote::ToTokens;
use quote::TokenStreamExt;
use std::collections::BTreeMap;
use strum::IntoEnumIterator;
use strum::IntoStaticStr;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use syn2::Attribute;
use syn2::FnArg;
use syn2::GenericParam;
use syn2::Generics;
use syn2::Pat;
use syn2::Path;
use syn2::Signature;
use syn2::Type;
use syn2::TypePath;
use thiserror::Error;

use super::signature_retval::parse_return;

#[allow(non_camel_case_types)]
#[derive(
  Copy, Clone, Debug, Eq, PartialEq, IntoStaticStr, EnumString, EnumIter,
)]
pub enum NumericArg {
  /// A placeholder argument for arguments annotated with #[smi].
  __SMI__,
  /// A placeholder argument for void data.
  __VOID__,
  bool,
  i8,
  u8,
  i16,
  u16,
  i32,
  u32,
  i64,
  u64,
  f32,
  f64,
  isize,
  usize,
}

impl NumericArg {
  /// Returns the primary mapping from this primitive to an associated V8 typed array.
  pub fn v8_array_type(self) -> Option<V8Arg> {
    use NumericArg::*;
    use V8Arg::*;
    Some(match self {
      i8 => Int8Array,
      u8 => Uint8Array,
      i16 => Int16Array,
      u16 => Uint16Array,
      i32 => Int32Array,
      u32 => Uint32Array,
      i64 => BigInt64Array,
      u64 => BigUint64Array,
      f32 => Float32Array,
      f64 => Float64Array,
      _ => return None,
    })
  }
}

impl ToTokens for NumericArg {
  fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
    let ident = Ident::new(self.into(), Span::call_site());
    tokens.extend(quote! { #ident })
  }
}

#[derive(
  Copy, Clone, Debug, Eq, PartialEq, IntoStaticStr, EnumString, EnumIter,
)]
pub enum V8Arg {
  Value,
  External,
  Object,
  Array,
  ArrayBuffer,
  ArrayBufferView,
  DataView,
  TypedArray,
  BigInt64Array,
  BigUint64Array,
  Float32Array,
  Float64Array,
  Int16Array,
  Int32Array,
  Int8Array,
  Uint16Array,
  Uint32Array,
  Uint8Array,
  Uint8ClampedArray,
  BigIntObject,
  BooleanObject,
  Date,
  Function,
  Map,
  NumberObject,
  Promise,
  PromiseResolver,
  Proxy,
  RegExp,
  Set,
  SharedArrayBuffer,
  StringObject,
  SymbolObject,
  WasmMemoryObject,
  WasmModuleObject,
  Primitive,
  BigInt,
  Boolean,
  Name,
  String,
  Symbol,
  Number,
  Integer,
  Int32,
  Uint32,
}

impl ToTokens for V8Arg {
  fn to_tokens(&self, tokens: &mut TokenStream) {
    let v8: &'static str = self.into();
    tokens.append(format_ident!("{v8}"))
  }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Special {
  HandleScope,
  OpState,
  String,
  CowStr,
  RefStr,
  FastApiCallbackOptions,
}

/// Buffers are complicated and may be shared/owned, shared/unowned, a copy, or detached.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Buffer {
  /// Shared/unowned, may be resizable. [`&[u8]`], [`&mut [u8]`], [`&[u32]`], etc...
  Slice(RefType, NumericArg),
  /// Shared/unowned, may be resizable. [`*const u8`], [`*mut u8`], [`*const u32`], etc...
  Ptr(RefType, NumericArg),
  /// Owned, copy. [`Box<[u8]>`], [`Box<[u32]>`], etc...
  BoxSlice(NumericArg),
  /// Owned, copy. [`Vec<u8>`], [`Vec<u32>`], etc...
  Vec(NumericArg),
  /// Maybe shared or a copy. Stored in `bytes::Bytes`
  Bytes,
  /// Shared, not resizable (or resizable and detatched), stored in `serde_v8::V8Slice`
  V8Slice,
  /// Shared, not resizable (or resizable and detatched), stored in `serde_v8::JSBuffer`
  JSBuffer,
}

impl Buffer {
  fn is_valid_mode(&self, mode: BufferMode) -> bool {
    match self {
      Buffer::Bytes => matches!(mode, BufferMode::Copy),
      Buffer::JSBuffer => matches!(
        mode,
        BufferMode::Copy | BufferMode::Detach | BufferMode::Unsafe
      ),
      Buffer::V8Slice => matches!(
        mode,
        BufferMode::Copy | BufferMode::Detach | BufferMode::Unsafe
      ),
      Buffer::Vec(..) => matches!(mode, BufferMode::Copy),
      Buffer::BoxSlice(..) => matches!(mode, BufferMode::Copy),
      Buffer::Slice(..) => {
        matches!(mode, BufferMode::Detach | BufferMode::Unsafe)
      }
      Buffer::Ptr(..) => {
        matches!(mode, BufferMode::Detach | BufferMode::Unsafe)
      }
    }
  }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum External {
  /// c_void
  Ptr(RefType),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RefType {
  Ref,
  Mut,
}

/// Args are not a 1:1 mapping with Rust types, rather they represent broad classes of types that
/// tend to have similar argument handling characteristics. This may need one more level of indirection
/// given how many of these types have option variants, however.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Arg {
  Void,
  Special(Special),
  Buffer(Buffer),
  External(External),
  Ref(RefType, Special),
  RcRefCell(Special),
  Option(Special),
  OptionNumeric(NumericArg),
  OptionV8Local(V8Arg),
  V8Local(V8Arg),
  V8Global(V8Arg),
  OptionV8Ref(RefType, V8Arg),
  V8Ref(RefType, V8Arg),
  Numeric(NumericArg),
  SerdeV8(String),
  State(RefType, String),
  OptionState(RefType, String),
}

impl Arg {
  /// Is this argument virtual? ie: does it come from the æther rather than a concrete JavaScript input
  /// argument?
  #[allow(clippy::match_like_matches_macro)]
  pub fn is_virtual(&self) -> bool {
    match self {
      Self::Special(
        Special::FastApiCallbackOptions
        | Special::OpState
        | Special::HandleScope,
      ) => true,
      Self::Ref(
        _,
        Special::FastApiCallbackOptions
        | Special::OpState
        | Special::HandleScope,
      ) => true,
      Self::RcRefCell(
        Special::FastApiCallbackOptions
        | Special::OpState
        | Special::HandleScope,
      ) => true,
      Self::State(..) | Self::OptionState(..) => true,
      _ => false,
    }
  }

  /// Convert the [`Arg`] into a [`TokenStream`] representing the fully-qualified type.
  #[allow(unused)] // unused for now but keeping
  pub fn type_token(&self, deno_core: &TokenStream) -> TokenStream {
    match self {
      Arg::V8Ref(RefType::Ref, v8) => quote!(&#deno_core::v8::#v8),
      Arg::V8Ref(RefType::Mut, v8) => quote!(&mut #deno_core::v8::#v8),
      Arg::V8Local(v8) => quote!(#deno_core::v8::Local<#deno_core::v8::#v8>),
      Arg::OptionV8Ref(RefType::Ref, v8) => {
        quote!(::std::option::Option<&#deno_core::v8::#v8>)
      }
      Arg::OptionV8Ref(RefType::Mut, v8) => {
        quote!(::std::option::Option<&mut #deno_core::v8::#v8>)
      }
      Arg::OptionV8Local(v8) => {
        quote!(::std::option::Option<#deno_core::v8::Local<#deno_core::v8::#v8>>)
      }
      _ => todo!(),
    }
  }

  /// Is this type an [`Option`]?
  pub fn is_option(&self) -> bool {
    matches!(
      self,
      Arg::OptionV8Ref(..)
        | Arg::OptionV8Local(..)
        | Arg::OptionNumeric(..)
        | Arg::Option(..)
        | Arg::OptionState(..)
    )
  }
}

pub enum ParsedType {
  TSpecial(Special),
  TBuffer(Buffer),
  TV8(V8Arg),
  // TODO(mmastrac): We need to carry the mut status through somehow
  TV8Mut(V8Arg),
  TNumeric(NumericArg),
}

pub enum ParsedTypeContainer {
  CBare(ParsedType),
  COption(ParsedType),
  CRcRefCell(ParsedType),
  COptionV8Local(ParsedType),
  CV8Local(ParsedType),
  CV8Global(ParsedType),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RetVal {
  /// An op that can never fail.
  Infallible(Arg),
  /// An op returning Result<Something, ...>
  Result(Arg),
  /// An op returning a future, either `async fn() -> Something` or `fn() -> impl Future<Output = Something>`.
  Future(Arg),
  /// An op returning a future with a result, either `async fn() -> Result<Something, ...>`
  /// or `fn() -> impl Future<Output = Result<Something, ...>>`.
  FutureResult(Arg),
  /// An op returning a result future: `fn() -> Result<impl Future<Output = Something>>`,
  /// allowing it to exit before starting any async work.
  ResultFuture(Arg),
  /// An op returning a result future of a result: `fn() -> Result<impl Future<Output = Result<Something, ...>>>`,
  /// allowing it to exit before starting any async work.
  ResultFutureResult(Arg),
}

impl RetVal {
  pub fn is_async(&self) -> bool {
    use RetVal::*;
    matches!(
      self,
      Future(..) | FutureResult(..) | ResultFuture(..) | ResultFutureResult(..)
    )
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedSignature {
  // The parsed arguments
  pub args: Vec<Arg>,
  // The argument names
  pub names: Vec<String>,
  // The parsed return value
  pub ret_val: RetVal,
  // One and only one lifetime allowed
  pub lifetime: Option<String>,
  // Generic bounds: each generic must have one and only simple trait bound
  pub generic_bounds: BTreeMap<String, String>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BufferMode {
  /// Unsafely shared buffers that may possibly change on the JavaScript side upon re-entry into
  /// V8. Rust code should not treat these as traditional buffers.
  Unsafe,
  /// Shared buffers that are copied from V8 unconditionally. May be expensive, but these
  /// buffers are guaranteed to be owned by Rust.
  Copy,
  /// Buffers that are detached and owned purely by Rust. JavaScript will no longer have
  /// access to these buffers and will see zero-sized buffers rather than the contents
  /// that were passed in here.
  Detach,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AttributeModifier {
  /// #[serde], for serde_v8 types.
  Serde,
  /// #[smi], for non-integral ID types representing small integers (-2³¹ and 2³¹-1 on 64-bit platforms,
  /// see https://medium.com/fhinkel/v8-internals-how-small-is-a-small-integer-e0badc18b6da).
  Smi,
  /// #[string], for strings.
  String,
  /// #[state], for automatic OpState extraction.
  State,
  /// #[buffer], for buffers.
  Buffer(BufferMode),
}

#[derive(Error, Debug)]
pub enum SignatureError {
  #[error("Invalid argument: '{0}'")]
  ArgError(String, #[source] ArgError),
  #[error("Invalid return type")]
  RetError(#[from] RetError),
  #[error("Only one lifetime is permitted")]
  TooManyLifetimes,
  #[error("Generic '{0}' must have one and only bound (either <T> and 'where T: Trait', or <T: Trait>)")]
  GenericBoundCardinality(String),
  #[error("Where clause predicate '{0}' (eg: where T: Trait) must appear in generics list (eg: <T>)")]
  WherePredicateMustAppearInGenerics(String),
  #[error("All generics must appear only once in the generics parameter list or where clause")]
  DuplicateGeneric(String),
  #[error("Generic lifetime '{0}' may not have bounds (eg: <'a: 'b>)")]
  LifetimesMayNotHaveBounds(String),
  #[error("Invalid generic: '{0}' Only simple generics bounds are allowed (eg: T: Trait)")]
  InvalidGeneric(String),
  #[error("Invalid predicate: '{0}' Only simple where predicates are allowed (eg: T: Trait)")]
  InvalidWherePredicate(String),
  #[error("State may be either a single OpState parameter, one mutable #[state], or multiple immultiple #[state]s")]
  InvalidOpStateCombination,
}

#[derive(Error, Debug)]
pub enum AttributeError {
  #[error("Unknown or invalid attribute '{0}'")]
  InvalidAttribute(String),
  #[error("Too many attributes")]
  TooManyAttributes,
}

#[derive(Error, Debug)]
pub enum ArgError {
  #[error("Invalid self argument")]
  InvalidSelf,
  #[error("Invalid argument type: {0}")]
  InvalidType(String),
  #[error("Invalid numeric argument type: {0}")]
  InvalidNumericType(String),
  #[error(
    "Invalid argument type path (should this be #[smi] or #[serde]?): {0}"
  )]
  InvalidTypePath(String),
  #[error("The type {0} cannot be a reference")]
  InvalidReference(String),
  #[error("The type {0} must be a reference")]
  MissingReference(String),
  #[error("Invalid or deprecated #[serde] type '{0}': {1}")]
  InvalidSerdeType(String, &'static str),
  #[error("Invalid #[string] type: {0}")]
  InvalidStringType(String),
  #[error("Invalid #[buffer] type: {0}")]
  InvalidBufferType(String),
  #[error("Invalid #[buffer] mode {0} for {1}")]
  InvalidBufferMode(String, String),
  #[error("Cannot use #[serde] for type: {0}")]
  InvalidSerdeAttributeType(String),
  #[error("Invalid v8 type: {0}")]
  InvalidV8Type(String),
  #[error("Internal error: {0}")]
  InternalError(String),
  #[error("Missing a #[string] attribute")]
  MissingStringAttribute,
  #[error("Missing a #[buffer] attribute")]
  MissingBufferAttribute,
  #[error("Invalid #[state] type '{0}'")]
  InvalidStateType(String),
  #[error("Argument attribute error")]
  AttributeError(#[from] AttributeError),
}

#[derive(Error, Debug)]
pub enum RetError {
  #[error("Invalid return type")]
  InvalidType(#[from] ArgError),
  #[error("Return value attribute error")]
  AttributeError(#[from] AttributeError),
}

#[derive(Copy, Clone, Default)]
pub(crate) struct Attributes {
  primary: Option<AttributeModifier>,
}

impl Attributes {
  pub fn string() -> Self {
    Self {
      primary: Some(AttributeModifier::String),
    }
  }
}

pub(crate) fn stringify_token(tokens: impl ToTokens) -> String {
  tokens
    .into_token_stream()
    .into_iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>()
    .join("")
    // Ick.
    // TODO(mmastrac): Should we pretty-format this instead?
    .replace(" , ", ", ")
}

pub fn parse_signature(
  attributes: Vec<Attribute>,
  signature: Signature,
) -> Result<ParsedSignature, SignatureError> {
  let mut args = vec![];
  let mut names = vec![];
  for input in signature.inputs {
    let name = match &input {
      FnArg::Receiver(_) => "self".to_owned(),
      FnArg::Typed(ty) => match &*ty.pat {
        Pat::Ident(ident) => ident.ident.to_string(),
        _ => "(complex)".to_owned(),
      },
    };
    names.push(name.clone());
    args.push(
      parse_arg(input).map_err(|err| SignatureError::ArgError(name, err))?,
    );
  }
  let ret_val = parse_return(
    signature.asyncness.is_some(),
    parse_attributes(&attributes).map_err(RetError::AttributeError)?,
    &signature.output,
  )?;
  let lifetime = parse_lifetime(&signature.generics)?;
  let generic_bounds = parse_generics(&signature.generics)?;

  let mut has_opstate = false;
  let mut has_mut_state = false;
  let mut has_ref_state = false;

  for arg in &args {
    match arg {
      Arg::RcRefCell(Special::OpState) | Arg::Ref(_, Special::OpState) => {
        has_opstate = true
      }
      Arg::State(RefType::Ref, _) | Arg::OptionState(RefType::Ref, _) => {
        has_ref_state = true
      }
      Arg::State(RefType::Mut, _) | Arg::OptionState(RefType::Mut, _) => {
        if has_mut_state {
          return Err(SignatureError::InvalidOpStateCombination);
        }
        has_mut_state = true;
      }
      _ => {}
    }
  }

  // Ensure that either zero or one and only one of these are true
  if has_opstate as u8 + has_mut_state as u8 + has_ref_state as u8 > 1 {
    return Err(SignatureError::InvalidOpStateCombination);
  }

  Ok(ParsedSignature {
    args,
    names,
    ret_val,
    lifetime,
    generic_bounds,
  })
}

/// Extract one lifetime from the [`syn2::Generics`], ensuring that the lifetime is valid
/// and has no bounds.
fn parse_lifetime(
  generics: &Generics,
) -> Result<Option<String>, SignatureError> {
  let mut res = None;
  for param in &generics.params {
    if let GenericParam::Lifetime(lt) = param {
      if !lt.bounds.is_empty() {
        return Err(SignatureError::LifetimesMayNotHaveBounds(
          lt.lifetime.to_string(),
        ));
      }
      if res.is_some() {
        return Err(SignatureError::TooManyLifetimes);
      }
      res = Some(lt.lifetime.ident.to_string());
    }
  }
  Ok(res)
}

/// Parse and validate generics. We require one and only one trait bound for each generic
/// parameter. Tries to sanity check and return reasonable errors for possible signature errors.
fn parse_generics(
  generics: &Generics,
) -> Result<BTreeMap<String, String>, SignatureError> {
  let mut where_clauses = BTreeMap::new();

  // First, extract the where clause so we can detect duplicated predicates
  if let Some(where_clause) = &generics.where_clause {
    for predicate in &where_clause.predicates {
      let predicate = predicate.to_token_stream();
      let (generic_name, bound) = std::panic::catch_unwind(|| {
        use syn2 as syn;
        rules!(predicate => {
          ($t:ident : $bound:path) => (t.to_string(), stringify_token(bound)),
        })
      })
      .map_err(|_| {
        SignatureError::InvalidWherePredicate(predicate.to_string())
      })?;
      if where_clauses.insert(generic_name.clone(), bound).is_some() {
        return Err(SignatureError::DuplicateGeneric(generic_name));
      }
    }
  }

  let mut res = BTreeMap::new();
  for param in &generics.params {
    if let GenericParam::Type(ty) = param {
      let ty = ty.to_token_stream();
      let (name, bound) = std::panic::catch_unwind(|| {
        use syn2 as syn;
        rules!(ty => {
          ($t:ident : $bound:path) => (t.to_string(), Some(stringify_token(bound))),
          ($t:ident) => (t.to_string(), None),
        })
      }).map_err(|_| SignatureError::InvalidGeneric(ty.to_string()))?;
      let bound = match bound {
        Some(bound) => {
          if where_clauses.contains_key(&name) {
            return Err(SignatureError::GenericBoundCardinality(name));
          }
          bound
        }
        None => {
          let Some(bound) = where_clauses.remove(&name) else {
            return Err(SignatureError::GenericBoundCardinality(name));
          };
          bound
        }
      };
      if res.contains_key(&name) {
        return Err(SignatureError::DuplicateGeneric(name));
      }
      res.insert(name, bound);
    }
  }
  if !where_clauses.is_empty() {
    return Err(SignatureError::WherePredicateMustAppearInGenerics(
      where_clauses.into_keys().next().unwrap(),
    ));
  }

  Ok(res)
}

fn parse_attributes(
  attributes: &[Attribute],
) -> Result<Attributes, AttributeError> {
  let mut attrs = vec![];
  for attr in attributes {
    if let Some(attr) = parse_attribute(attr)? {
      attrs.push(attr)
    }
  }

  if attrs.is_empty() {
    return Ok(Attributes::default());
  }
  if attrs.len() > 1 {
    return Err(AttributeError::TooManyAttributes);
  }
  Ok(Attributes {
    primary: Some(*attrs.get(0).unwrap()),
  })
}

/// Is this a special attribute that we understand?
pub fn is_attribute_special(attr: &Attribute) -> bool {
  parse_attribute(attr).unwrap_or_default().is_some()
}

/// Parses an attribute, returning None if this is an attribute we support but is
/// otherwise unknown (ie: doc comments).
fn parse_attribute(
  attr: &Attribute,
) -> Result<Option<AttributeModifier>, AttributeError> {
  let tokens = attr.into_token_stream();
  let res = std::panic::catch_unwind(|| {
    use syn2 as syn;
    rules!(tokens => {
      (#[serde]) => Some(AttributeModifier::Serde),
      (#[smi]) => Some(AttributeModifier::Smi),
      (#[string]) => Some(AttributeModifier::String),
      (#[state]) => Some(AttributeModifier::State),
      (#[buffer]) => Some(AttributeModifier::Buffer(BufferMode::Unsafe)),
      (#[buffer(unsafe)]) => Some(AttributeModifier::Buffer(BufferMode::Unsafe)),
      (#[buffer(copy)]) => Some(AttributeModifier::Buffer(BufferMode::Copy)),
      (#[buffer(detach)]) => Some(AttributeModifier::Buffer(BufferMode::Detach)),
      (#[allow ($_rule:path)]) => None,
      (#[doc = $_attr:literal]) => None,
    })
  }).map_err(|_| AttributeError::InvalidAttribute(stringify_token(attr)))?;
  Ok(res)
}

fn parse_numeric_type(tp: &Path) -> Result<NumericArg, ArgError> {
  if tp.segments.len() == 1 {
    let segment = tp.segments.first().unwrap().ident.to_string();
    for numeric in NumericArg::iter() {
      if Into::<&'static str>::into(numeric) == segment.as_str() {
        return Ok(numeric);
      }
    }
  }

  Err(ArgError::InvalidNumericType(stringify_token(tp)))
}

/// Parse a raw type into a container + type, allowing us to simplify the typechecks elsewhere in
/// this code.
fn parse_type_path(
  attrs: Attributes,
  is_ref: bool,
  tp: &TypePath,
) -> Result<ParsedTypeContainer, ArgError> {
  use ParsedType::*;
  use ParsedTypeContainer::*;

  use syn2 as syn;

  let tokens = tp.clone().into_token_stream();
  let res = if let Ok(numeric) = parse_numeric_type(&tp.path) {
    CBare(TNumeric(numeric))
  } else {
    std::panic::catch_unwind(|| {
    rules!(tokens => {
      ( $( std :: str  :: )? String ) => {
        Ok(CBare(TSpecial(Special::String)))
      }
      // Note that the reference is checked below
      ( $( std :: str :: )? str ) => {
        Ok(CBare(TSpecial(Special::RefStr)))
      }
      ( $( std :: borrow :: )? Cow < str > ) => {
        Ok(CBare(TSpecial(Special::CowStr)))
      }
      ( $( std :: vec ::)? Vec < $ty:path > ) => {
        Ok(CBare(TBuffer(Buffer::Vec(parse_numeric_type(&ty)?))))
      }
      ( $( std :: boxed ::)? Box < [ $ty:path ] > ) => {
        Ok(CBare(TBuffer(Buffer::BoxSlice(parse_numeric_type(&ty)?))))
      }
      ( $( serde_v8 :: )? V8Slice ) => {
        Ok(CBare(TBuffer(Buffer::V8Slice)))
      }
      ( $( serde_v8 :: )? JSBuffer ) => {
        Ok(CBare(TBuffer(Buffer::JSBuffer)))
      }
      ( $( bytes :: )? Bytes ) => {
        Ok(CBare(TBuffer(Buffer::Bytes)))
      }
      ( $( std :: ffi :: )? c_void ) => Ok(CBare(TNumeric(NumericArg::__VOID__))),
      ( OpState ) => Ok(CBare(TSpecial(Special::OpState))),
      ( v8 :: HandleScope $( < $_scope:lifetime >)? ) => Ok(CBare(TSpecial(Special::HandleScope))),
      ( v8 :: FastApiCallbackOptions ) => Ok(CBare(TSpecial(Special::FastApiCallbackOptions))),
      ( v8 :: Local < $( $_scope:lifetime , )? v8 :: $v8:ident >) => Ok(CV8Local(TV8(parse_v8_type(&v8)?))),
      ( v8 :: Global < $( $_scope:lifetime , )? v8 :: $v8:ident >) => Ok(CV8Global(TV8(parse_v8_type(&v8)?))),
      ( v8 :: $v8:ident ) => Ok(CBare(TV8(parse_v8_type(&v8)?))),
      ( Rc < RefCell < $ty:ty > > ) => Ok(CRcRefCell(TSpecial(parse_type_special(attrs, &ty)?))),
      ( Option < $ty:ty > ) => {
        match parse_type(attrs, &ty)? {
          Arg::Special(special) => Ok(COption(TSpecial(special))),
          Arg::Numeric(numeric) => Ok(COption(TNumeric(numeric))),
          Arg::Buffer(buffer) => Ok(COption(TBuffer(buffer))),
          Arg::V8Ref(RefType::Ref, v8) => Ok(COption(TV8(v8))),
          Arg::V8Ref(RefType::Mut, v8) => Ok(COption(TV8Mut(v8))),
          Arg::V8Local(v8) => Ok(COptionV8Local(TV8(v8))),
          _ => Err(ArgError::InvalidType(stringify_token(ty)))
        }
      }
      ( $any:ty ) => Err(ArgError::InvalidTypePath(stringify_token(any))),
    })
  }).map_err(|e| ArgError::InternalError(format!("parse_type_path {e:?}")))??
  };

  // Ensure that we have the correct reference state. This is a bit awkward but it's
  // the easiest way to work with the 'rules!' macro above.
  match res {
    // OpState appears in both ways
    CBare(TSpecial(Special::OpState)) => {}
    CBare(TSpecial(Special::RefStr | Special::HandleScope) | TV8(_)) => {
      if !is_ref {
        return Err(ArgError::MissingReference(stringify_token(tp)));
      }
    }
    _ => {
      if is_ref {
        return Err(ArgError::InvalidReference(stringify_token(tp)));
      }
    }
  }

  match res {
    CBare(TSpecial(Special::RefStr | Special::CowStr | Special::String)) => {
      if attrs.primary != Some(AttributeModifier::String) {
        return Err(ArgError::MissingStringAttribute);
      }
    }
    CBare(TBuffer(buffer)) => {
      if let Some(AttributeModifier::Buffer(mode)) = attrs.primary {
        if !buffer.is_valid_mode(mode) {
          return Err(ArgError::InvalidBufferMode(
            format!("{mode:?}"),
            stringify_token(tp),
          ));
        }
      } else {
        return Err(ArgError::MissingBufferAttribute);
      }
    }
    CBare(_) => {
      if attrs.primary == Some(AttributeModifier::String) {
        return Err(ArgError::InvalidStringType(stringify_token(tp)));
      }
      if let Some(AttributeModifier::Buffer(_)) = attrs.primary {
        return Err(ArgError::InvalidBufferType(stringify_token(tp)));
      }
    }
    _ => {
      // Ignore for other containers
    }
  }

  Ok(res)
}

fn parse_v8_type(v8: &Ident) -> Result<V8Arg, ArgError> {
  let v8 = v8.to_string();
  V8Arg::try_from(v8.as_str()).map_err(|_| ArgError::InvalidV8Type(v8))
}

fn parse_type_special(
  attrs: Attributes,
  ty: &Type,
) -> Result<Special, ArgError> {
  match parse_type(attrs, ty)? {
    Arg::Special(special) => Ok(special),
    _ => Err(ArgError::InvalidType(stringify_token(ty))),
  }
}

fn parse_type_state(ty: &Type) -> Result<Arg, ArgError> {
  let s = match ty {
    Type::Path(of) => {
      let inner_type = std::panic::catch_unwind(|| {
        use syn2 as syn;
        rules!(of.into_token_stream() => {
          (Option< $ty:ty >) => ty,
        })
      })
      .map_err(|_| ArgError::InvalidStateType(stringify_token(ty)))?;
      match parse_type_state(&inner_type)? {
        Arg::State(reftype, state) => Arg::OptionState(reftype, state),
        _ => return Err(ArgError::InvalidStateType(stringify_token(ty))),
      }
    }
    Type::Reference(of) => {
      if of.mutability.is_some() {
        Arg::State(RefType::Mut, stringify_token(&of.elem))
      } else {
        Arg::State(RefType::Ref, stringify_token(&of.elem))
      }
    }
    _ => return Err(ArgError::InvalidStateType(stringify_token(ty))),
  };
  Ok(s)
}

pub(crate) fn parse_type(
  attrs: Attributes,
  ty: &Type,
) -> Result<Arg, ArgError> {
  use ParsedType::*;
  use ParsedTypeContainer::*;

  if let Some(primary) = attrs.primary {
    match primary {
      AttributeModifier::Serde => match ty {
        Type::Tuple(of) => {
          return Ok(Arg::SerdeV8(stringify_token(of)));
        }
        Type::Path(of) => {
          // If this type will parse without #[serde] (or with #[string]), it is illegal to use this type with #[serde]
          if parse_type_path(Attributes::default(), false, of).is_ok() {
            return Err(ArgError::InvalidSerdeAttributeType(stringify_token(
              ty,
            )));
          }
          // If this type will parse without #[serde] (or with #[string]), it is illegal to use this type with #[serde]
          if parse_type_path(Attributes::string(), false, of).is_ok() {
            return Err(ArgError::InvalidSerdeAttributeType(stringify_token(
              ty,
            )));
          }

          // Denylist of serde_v8 types with better alternatives
          let ty = of.into_token_stream();
          let token = stringify_token(of.path.clone());
          if let Ok(Some(err)) = std::panic::catch_unwind(|| {
            use syn2 as syn;
            rules!(ty => {
              ( $( serde_v8:: )? Value $( < $_lifetime:lifetime >)? ) => Some("use v8::Value"),
              ( $_ty:ty ) => None,
            })
          }) {
            return Err(ArgError::InvalidSerdeType(stringify_token(ty), err));
          }

          return Ok(Arg::SerdeV8(token));
        }
        _ => {
          return Err(ArgError::InvalidSerdeAttributeType(stringify_token(ty)))
        }
      },
      AttributeModifier::State => {
        return parse_type_state(ty);
      }
      AttributeModifier::String => {
        // We handle this as part of the normal parsing process
      }
      AttributeModifier::Buffer(_) => {
        // We handle this as part of the normal parsing process
      }
      AttributeModifier::Smi => {
        return Ok(Arg::Numeric(NumericArg::__SMI__));
      }
    }
  };
  match ty {
    Type::Tuple(of) => {
      if of.elems.is_empty() {
        Ok(Arg::Void)
      } else {
        Err(ArgError::InvalidType(stringify_token(ty)))
      }
    }
    Type::Reference(of) => {
      let mut_type = if of.mutability.is_some() {
        RefType::Mut
      } else {
        RefType::Ref
      };
      match &*of.elem {
        // Note that we only allow numeric slices here -- if we decide to allow slices of things like v8 values,
        // this branch will need to be re-written.
        Type::Slice(of) => {
          if let Type::Path(path) = &*of.elem {
            match parse_numeric_type(&path.path)? {
              NumericArg::__VOID__ => {
                Ok(Arg::External(External::Ptr(mut_type)))
              }
              numeric => {
                if let Some(AttributeModifier::Buffer(mode)) = attrs.primary {
                  let buffer = Buffer::Slice(mut_type, numeric);
                  if !buffer.is_valid_mode(mode) {
                    return Err(ArgError::InvalidBufferMode(
                      format!("{mode:?}"),
                      stringify_token(ty),
                    ));
                  }
                  Ok(Arg::Buffer(buffer))
                } else {
                  Err(ArgError::InvalidBufferType(stringify_token(ty)))
                }
              }
            }
          } else {
            Err(ArgError::InvalidType(stringify_token(ty)))
          }
        }
        Type::Path(of) => match parse_type_path(attrs, true, of)? {
          CBare(TSpecial(Special::RefStr)) => Ok(Arg::Special(Special::RefStr)),
          COption(TSpecial(Special::RefStr)) => {
            Ok(Arg::Option(Special::RefStr))
          }
          CBare(TV8(v8)) => Ok(Arg::V8Ref(mut_type, v8)),
          CBare(TSpecial(special)) => Ok(Arg::Ref(mut_type, special)),
          _ => Err(ArgError::InvalidType(stringify_token(ty))),
        },
        _ => Err(ArgError::InvalidType(stringify_token(ty))),
      }
    }
    Type::Ptr(of) => {
      let mut_type = if of.mutability.is_some() {
        RefType::Mut
      } else {
        RefType::Ref
      };
      match &*of.elem {
        Type::Path(of) => match parse_type_path(attrs, false, of)? {
          CBare(TNumeric(numeric)) if numeric == NumericArg::__VOID__ => {
            Ok(Arg::External(External::Ptr(mut_type)))
          }
          CBare(TNumeric(numeric)) => {
            if let Some(AttributeModifier::Buffer(mode)) = attrs.primary {
              let buffer = Buffer::Ptr(mut_type, numeric);
              if !buffer.is_valid_mode(mode) {
                return Err(ArgError::InvalidBufferMode(
                  format!("{mode:?}"),
                  stringify_token(ty),
                ));
              }
              Ok(Arg::Buffer(buffer))
            } else {
              Err(ArgError::InvalidBufferType(stringify_token(ty)))
            }
          }
          _ => Err(ArgError::InvalidType(stringify_token(ty))),
        },
        _ => Err(ArgError::InvalidType(stringify_token(ty))),
      }
    }
    Type::Path(of) => match parse_type_path(attrs, false, of)? {
      CBare(TNumeric(numeric)) => Ok(Arg::Numeric(numeric)),
      CBare(TSpecial(special)) => Ok(Arg::Special(special)),
      CBare(TBuffer(buffer)) => Ok(Arg::Buffer(buffer)),
      COption(TNumeric(special)) => Ok(Arg::OptionNumeric(special)),
      COption(TSpecial(special)) => Ok(Arg::Option(special)),
      CRcRefCell(TSpecial(special)) => Ok(Arg::RcRefCell(special)),
      COptionV8Local(TV8(v8)) => Ok(Arg::OptionV8Local(v8)),
      COption(TV8(v8)) => Ok(Arg::OptionV8Ref(RefType::Ref, v8)),
      COption(TV8Mut(v8)) => Ok(Arg::OptionV8Ref(RefType::Mut, v8)),
      CV8Local(TV8(v8)) => Ok(Arg::V8Local(v8)),
      CV8Global(TV8(v8)) => Ok(Arg::V8Global(v8)),
      _ => Err(ArgError::InvalidType(stringify_token(ty))),
    },
    _ => Err(ArgError::InvalidType(stringify_token(ty))),
  }
}

fn parse_arg(arg: FnArg) -> Result<Arg, ArgError> {
  let FnArg::Typed(typed) = arg else {
    return Err(ArgError::InvalidSelf);
  };
  parse_type(parse_attributes(&typed.attrs)?, &typed.ty)
}

#[cfg(test)]
mod tests {
  use super::*;
  use syn2::parse_str;
  use syn2::ItemFn;

  // We can't test pattern args :/
  // https://github.com/rust-lang/rfcs/issues/2688
  macro_rules! test {
    (
      // Function attributes
      $(# [ $fn_attr:meta ])?
      // fn name < 'scope, GENERIC1, GENERIC2, ... >
      $(async fn $name1:ident)?
      $(fn $name2:ident)?
      $( < $scope:lifetime $( , $generic:ident)* >)?
      (
        // Argument attribute, argument
        $( $(# [ $attr:meta ])? $ident:ident : $ty:ty ),*
      )
      // Return value
      $(-> $(# [ $ret_attr:meta ])? $ret:ty)?
      // Where clause
      $( where $($trait:ident : $bounds:path),* )?
      ;
      // Expected return value
      $( < $( $lifetime_res:lifetime )? $(, $generic_res:ident : $bounds_res:path )* >)? ( $( $arg_res:expr ),* ) -> $ret_res:expr ) => {
      #[test]
      fn $($name1)? $($name2)? () {
        test(
          stringify!($( #[$fn_attr] )? $(async fn $name1)? $(fn $name2)? $( < $scope $( , $generic)* >)? ( $( $( #[$attr] )? $ident : $ty ),* ) $(-> $( #[$ret_attr] )? $ret)? $( where $($trait : $bounds),* )? {}),
          stringify!($( < $( $lifetime_res )? $(, $generic_res : $bounds_res)* > )?),
          stringify!($($arg_res),*),
          stringify!($ret_res)
        );
      }
    };
  }

  fn test(
    op: &str,
    generics_expected: &str,
    args_expected: &str,
    return_expected: &str,
  ) {
    // Parse the provided macro input as an ItemFn
    let item_fn = parse_str::<ItemFn>(op)
      .unwrap_or_else(|_| panic!("Failed to parse {op} as a ItemFn"));

    let attrs = item_fn.attrs;
    let sig = parse_signature(attrs, item_fn.sig).unwrap_or_else(|err| {
      panic!("Failed to successfully parse signature from {op} ({err:?})")
    });
    println!("Raw parsed signatures = {sig:?}");

    let mut generics_res = vec![];
    if let Some(lifetime) = sig.lifetime {
      generics_res.push(format!("'{lifetime}"));
    }
    for (name, bounds) in sig.generic_bounds {
      generics_res.push(format!("{name} : {bounds}"));
    }
    if !generics_res.is_empty() {
      assert_eq!(
        generics_expected,
        format!("< {} >", generics_res.join(", "))
      );
    }
    assert_eq!(
      args_expected.replace('\n', " "),
      format!("{:?}", sig.args)
        .trim_matches(|c| c == '[' || c == ']')
        .replace('\n', " ")
        .replace('"', "")
    );
    assert_eq!(
      return_expected,
      format!("{:?}", sig.ret_val).replace('"', "")
    );
  }

  macro_rules! expect_fail {
    ($name:ident, $error:expr, $f:item) => {
      #[test]
      pub fn $name() {
        expect_fail(stringify!($f), stringify!($error));
      }
    };
  }

  fn expect_fail(op: &str, error: &str) {
    // Parse the provided macro input as an ItemFn
    let item_fn = parse_str::<ItemFn>(op)
      .unwrap_or_else(|_| panic!("Failed to parse {op} as a ItemFn"));
    let attrs = item_fn.attrs;
    let err = parse_signature(attrs, item_fn.sig)
      .expect_err("Expected function to fail to parse");
    assert_eq!(format!("{err:?}"), error.to_owned());
  }

  test!(
    fn op_state_and_number(opstate: &mut OpState, a: u32) -> ();
    (Ref(Mut, OpState), Numeric(u32)) -> Infallible(Void)
  );
  test!(
    fn op_slices(#[buffer] r#in: &[u8], #[buffer] out: &mut [u8]);
    (Buffer(Slice(Ref, u8)), Buffer(Slice(Mut, u8))) -> Infallible(Void)
  );
  test!(
    #[serde] fn op_serde(#[serde] input: package::SerdeInputType) -> Result<package::SerdeReturnType, Error>;
    (SerdeV8(package::SerdeInputType)) -> Result(SerdeV8(package::SerdeReturnType))
  );
  test!(
    #[serde] fn op_serde_tuple(#[serde] input: (A, B)) -> (A, B);
    (SerdeV8((A, B))) -> Infallible(SerdeV8((A, B)))
  );
  test!(
    fn op_local(input: v8::Local<v8::String>) -> Result<v8::Local<v8::String>, Error>;
    (V8Local(String)) -> Result(V8Local(String))
  );
  test!(
    fn op_resource(#[smi] rid: ResourceId, #[buffer] buffer: &[u8]);
    (Numeric(__SMI__), Buffer(Slice(Ref, u8))) ->  Infallible(Void)
  );
  test!(
    fn op_option_numeric_result(state: &mut OpState) -> Result<Option<u32>, AnyError>;
    (Ref(Mut, OpState)) -> Result(OptionNumeric(u32))
  );
  test!(
    fn op_ffi_read_f64(state: &mut OpState, ptr: * mut c_void, offset: isize) -> Result <f64, AnyError>;
    (Ref(Mut, OpState), External(Ptr(Mut)), Numeric(isize)) -> Result(Numeric(f64))
  );
  test!(
    fn op_print(#[string] msg: &str, is_err: bool) -> Result<(), Error>;
    (Special(RefStr), Numeric(bool)) -> Result(Void)
  );
  test!(
    #[string] fn op_lots_of_strings(#[string] s: String, #[string] s2: Option<String>, #[string] s3: Cow<str>) -> String;
    (Special(String), Option(String), Special(CowStr)) -> Infallible(Special(String))
  );
  test!(
    #[string] fn op_lots_of_option_strings(#[string] s: Option<String>, #[string] s2: Option<&str>, #[string] s3: Option<Cow<str>>) -> Option<String>;
    (Option(String), Option(RefStr), Option(CowStr)) -> Infallible(Option(String))
  );
  test!(
    fn op_scope<'s>(#[string] msg: &'s str);
    <'s> (Special(RefStr)) -> Infallible(Void)
  );
  test!(
    fn op_scope_and_generics<'s, AB, BC>(#[string] msg: &'s str) where AB: some::Trait, BC: OtherTrait;
    <'s, AB: some::Trait, BC: OtherTrait> (Special(RefStr)) -> Infallible(Void)
  );
  test!(
    fn op_v8_types(s: &mut v8::String, sopt: Option<&mut v8::String>, s2: v8::Local<v8::String>, s3: v8::Global<v8::String>);
    (V8Ref(Mut, String), OptionV8Ref(Mut, String), V8Local(String), V8Global(String)) -> Infallible(Void)
  );
  test!(
    fn op_v8_scope<'s>(scope: &mut v8::HandleScope<'s>);
    <'s> (Ref(Mut, HandleScope)) -> Infallible(Void)
  );
  test!(
    fn op_state_rc(state: Rc<RefCell<OpState>>);
    (RcRefCell(OpState)) -> Infallible(Void)
  );
  test!(
    fn op_state_ref(state: &OpState);
    (Ref(Ref, OpState)) -> Infallible(Void)
  );
  test!(
    fn op_state_attr(#[state] something: &Something, #[state] another: Option<&Something>);
    (State(Ref, Something), OptionState(Ref, Something)) -> Infallible(Void)
  );
  test!(
    #[buffer(copy)] fn op_buffers(#[buffer(copy)] a: Vec<u8>, #[buffer(copy)] b: Box<[u8]>, #[buffer(copy)] c: bytes::Bytes, #[buffer] d: V8Slice, #[buffer] e: JSBuffer) -> Vec<u8>;
    (Buffer(Vec(u8)), Buffer(BoxSlice(u8)), Buffer(Bytes), Buffer(V8Slice), Buffer(JSBuffer)) -> Infallible(Buffer(Vec(u8)))
  );
  test!(
    async fn op_async_void();
    () -> Future(Void)
  );
  test!(
    async fn op_async_result_void() -> Result<()>;
    () -> FutureResult(Void)
  );
  test!(
    fn op_async_impl_void() -> impl Future<Output = ()>;
    () -> Future(Void)
  );
  test!(
    fn op_async_result_impl_void() -> Result<impl Future<Output = ()>, Error>;
    () -> ResultFuture(Void)
  );
  // Args

  expect_fail!(
    op_with_bad_string1,
    ArgError("s", MissingStringAttribute),
    fn f(s: &str) {}
  );
  expect_fail!(
    op_with_bad_string2,
    ArgError("s", MissingStringAttribute),
    fn f(s: String) {}
  );
  expect_fail!(
    op_with_bad_string3,
    ArgError("s", MissingStringAttribute),
    fn f(s: Cow<str>) {}
  );
  expect_fail!(
    op_with_invalid_string,
    ArgError("x", InvalidStringType("u32")),
    fn f(#[string] x: u32) {}
  );
  expect_fail!(
    op_with_invalid_buffer,
    ArgError("x", InvalidBufferType("u32")),
    fn f(#[buffer] x: u32) {}
  );
  expect_fail!(
    op_with_bad_attr,
    RetError(AttributeError(InvalidAttribute("#[badattr]"))),
    #[badattr]
    fn f() {}
  );
  expect_fail!(
    op_with_bad_attr2,
    ArgError("a", AttributeError(InvalidAttribute("#[badattr]"))),
    fn f(#[badattr] a: u32) {}
  );

  // Generics

  expect_fail!(op_with_two_lifetimes, TooManyLifetimes, fn f<'a, 'b>() {});
  expect_fail!(
    op_with_lifetime_bounds,
    LifetimesMayNotHaveBounds("'a"),
    fn f<'a: 'b, 'b>() {}
  );
  expect_fail!(
    op_with_missing_bounds,
    GenericBoundCardinality("B"),
    fn f<'a, B>() {}
  );
  expect_fail!(
    op_with_duplicate_bounds,
    GenericBoundCardinality("B"),
    fn f<'a, B: Trait>()
    where
      B: Trait,
    {
    }
  );
  expect_fail!(
    op_with_extra_bounds,
    WherePredicateMustAppearInGenerics("C"),
    fn f<'a, B>()
    where
      B: Trait,
      C: Trait,
    {
    }
  );

  expect_fail!(
    op_with_bad_serde_string,
    ArgError("s", InvalidSerdeAttributeType("String")),
    fn f(#[serde] s: String) {}
  );
  expect_fail!(
    op_with_bad_serde_str,
    ArgError("s", InvalidSerdeAttributeType("&str")),
    fn f(#[serde] s: &str) {}
  );
  expect_fail!(
    op_with_bad_serde_value,
    ArgError("v", InvalidSerdeType("serde_v8::Value", "use v8::Value")),
    fn f(#[serde] v: serde_v8::Value) {}
  );
}

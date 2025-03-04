// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.
use proc_macro2::Ident;
use proc_macro2::TokenStream;

pub struct GeneratorState {
  /// The path to the `deno_core` crate (either `deno_core` or `crate`, the latter used if the op is `(core)`).
  pub deno_core: TokenStream,

  /// Identifiers for each of the arguments of the original function
  pub args: Vec<Ident>,
  /// The new identifier for the original function's contents.
  pub call: Ident,
  /// The result of the `call` function
  pub result: Ident,

  /// The `v8::CallbackScope` used if necessary for the function.
  pub scope: Ident,
  /// The `v8::FunctionCallbackInfo` used to pass args into the slow function.
  pub info: Ident,
  /// The `v8::FunctionCallbackArguments` used to pass args into the slow function.
  pub fn_args: Ident,
  /// The `OpCtx` used for various information required for some ops.
  pub opctx: Ident,
  /// The `OpState` used for storing op state.
  pub opstate: Ident,
  /// The `FastApiCallbackOptions` used in fast calls for fallback returns.
  pub fast_api_callback_options: Ident,
  /// The `v8::ReturnValue` used in the slow function
  pub retval: Ident,
  /// The "slow" function (ie: the one that isn't a fastcall)
  pub slow_function: Ident,
  /// The "fast" function (ie: a fastcall)
  pub fast_function: Ident,
  /// The async function promise ID argument
  pub promise_id: Ident,

  pub needs_args: bool,
  pub needs_retval: bool,
  pub needs_scope: bool,
  pub needs_opstate: bool,
  pub needs_opctx: bool,
  pub needs_fast_opctx: bool,
  pub needs_fast_api_callback_options: bool,
}

/// Quotes a set of generator_state fields, along with variables captured from
/// the local environment.
///
/// Example: this will extract `deno_core`, `info` and `scope` from `generator_state`
/// before invoking the [`quote!`] macro.
///
/// ```nocompile
///  gs_quote!(generator_state(deno_core, info, scope) =>
///    (let mut #scope = unsafe { #deno_core::v8::CallbackScope::new(&*#info) };)
///  )
/// ```
macro_rules! gs_quote {
  ($generator_state:ident( $($idents:ident),* ) => $quotable:tt) => {
    {
      $(
        let $idents = &$generator_state.$idents;
      )*
      quote! $quotable
    }
  }
}

/// Extracts GeneratorState vars into the local scope.
///
/// Example:
///
/// Extracts `deno_core` from `generator_state` into a local variable. Equivalent to `let deno_core = &generator_state.deno_core`.
///
/// ```nocompile
/// gs_extract!(generator_state(deno_core))
/// ```
macro_rules! gs_extract {
  ($generator_state:ident( $($idents:ident),* )) => {
    $(
      let $idents = &$generator_state.$idents;
    )*
  }
}

pub(crate) use gs_extract;
pub(crate) use gs_quote;

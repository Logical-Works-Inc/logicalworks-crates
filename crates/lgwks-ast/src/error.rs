//! `error` owns this crate's typed-diagnostic definition macro.
//!
//! `std::error::Error` is a trait, and `ParseError` is built
//! as a typed diagnostic; code-observability consumers define or derive their
//! diagnostics from this stack.
//!
//! [`crate::define_error!`] allows defining typed error enums with `Display` and `Error`:
//!
//! ```
//! use lgwks_ast::define_error;
//!
//! define_error! {
//!     /// Diagnostic refusal.
//!     #[derive(Debug, PartialEq, Eq)]
//!     pub enum Diagnostic {
//!         /// Refused with reason.
//!         Refused(String) => "parse refused: {}",
//!     }
//! }
//! ```

/// Define a typed error enum implementing [`core::fmt::Display`] and [`std::error::Error`].
///
/// # Examples
///
/// ```
/// use lgwks_ast::define_error;
///
/// define_error! {
///     /// A diagnostic reported by analysis.
///     #[derive(Debug, PartialEq, Eq)]
///     pub enum Diagnostic {
///         /// A parse refusal.
///         Refused(String) => "parse refused: {}",
///     }
/// }
///
/// let diag = Diagnostic::Refused("bad syntax".to_string());
/// assert_eq!(diag.to_string(), "parse refused: bad syntax");
/// ```
#[macro_export]
macro_rules! define_error {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$v_meta:meta])*
                $variant:ident $( ( $($field_ty:ty),* $(,)? ) )? => $fmt:literal $( , )?
            )*
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $(
                $(#[$v_meta])*
                $variant $( ( $($field_ty),* ) )?,
            )*
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, __formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                $crate::define_error! {
                    @expand_match
                    $name, __formatter, self,
                    [];
                    $( [ $variant, $fmt, $( ( $($field_ty),* ) )? ] )*
                }
            }
        }

        impl ::std::error::Error for $name {}
    };

    (
        @expand_match
        $name:ident, $formatter:ident, $self:ident,
        [ $( $arms:tt )* ];
        [ $variant:ident, $fmt:literal, ] $( $rest:tt )*
    ) => {
        $crate::define_error! {
            @expand_match
            $name, $formatter, $self,
            [ $( $arms )* $name::$variant => ::core::write!($formatter, $fmt), ];
            $( $rest )*
        }
    };

    (
        @expand_match
        $name:ident, $formatter:ident, $self:ident,
        [ $( $arms:tt )* ];
        [ $variant:ident, $fmt:literal, ( $f0:ty ) ] $( $rest:tt )*
    ) => {
        $crate::define_error! {
            @expand_match
            $name, $formatter, $self,
            [ $( $arms )* $name::$variant(f0) => ::core::write!($formatter, $fmt, f0), ];
            $( $rest )*
        }
    };

    (
        @expand_match
        $name:ident, $formatter:ident, $self:ident,
        [ $( $arms:tt )* ];
        [ $variant:ident, $fmt:literal, ( $f0:ty, $f1:ty ) ] $( $rest:tt )*
    ) => {
        $crate::define_error! {
            @expand_match
            $name, $formatter, $self,
            [ $( $arms )* $name::$variant(f0, f1) => ::core::write!($formatter, $fmt, f0, f1), ];
            $( $rest )*
        }
    };

    (
        @expand_match
        $name:ident, $formatter:ident, $self:ident,
        [ $( $arms:tt )* ];
    ) => {
        match $self {
            $( $arms )*
        }
    };
}

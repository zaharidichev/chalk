#![deny(rust_2018_idioms)]

//! Contains the definition for the "Rust IR" -- this is basically a "lowered"
//! version of the AST, roughly corresponding to [the HIR] in the Rust
//! compiler.

use chalk_derive::{Fold, HasInterner};
use chalk_ir::cast::Cast;
use chalk_ir::fold::{shift::Shift, Fold, Folder};
use chalk_ir::interner::{HasInterner, Interner, TargetInterner};
use chalk_ir::{
    AliasEq, AliasTy, AssocTypeId, Binders, BoundVar, DebruijnIndex, ImplId, LifetimeData,
    Parameter, ParameterKind, QuantifiedWhereClause, StructId, Substitution, TraitId, TraitRef, Ty,
    TyData, TypeName, WhereClause,
};
use std::iter;

/// Identifier for an "associated type value" found in some impl.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssociatedTyValueId<I: Interner>(pub I::DefId);

chalk_ir::id_fold!(AssociatedTyValueId);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImplDatum<I: Interner> {
    pub polarity: Polarity,
    pub binders: Binders<ImplDatumBound<I>>,
    pub impl_type: ImplType,
    pub associated_ty_value_ids: Vec<AssociatedTyValueId<I>>,
}

impl<I: Interner> ImplDatum<I> {
    pub fn is_positive(&self) -> bool {
        self.polarity.is_positive()
    }

    pub fn trait_id(&self) -> TraitId<I> {
        self.binders.value.trait_ref.trait_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, HasInterner, Fold)]
pub struct ImplDatumBound<I: Interner> {
    pub trait_ref: TraitRef<I>,
    pub where_clauses: Vec<QuantifiedWhereClause<I>>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImplType {
    Local,
    External,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DefaultImplDatum<I: Interner> {
    pub binders: Binders<DefaultImplDatumBound<I>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DefaultImplDatumBound<I: Interner> {
    pub trait_ref: TraitRef<I>,
    pub accessible_tys: Vec<Ty<I>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructDatum<I: Interner> {
    pub binders: Binders<StructDatumBound<I>>,
    pub id: StructId<I>,
    pub flags: StructFlags,
}

impl<I: Interner> StructDatum<I> {
    pub fn name(&self, interner: &I) -> TypeName<I> {
        self.id.cast(interner)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Fold, HasInterner)]
pub struct StructDatumBound<I: Interner> {
    pub fields: Vec<Ty<I>>,
    pub where_clauses: Vec<QuantifiedWhereClause<I>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructFlags {
    pub upstream: bool,
    pub fundamental: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
/// A rust intermediate representation (rust_ir) of a Trait Definition. For
/// example, given the following rust code:
///
/// ```compile_fail
/// use std::fmt::Debug;
///
/// trait Foo<T>
/// where
///     T: Debug,
/// {
///     type Bar<U>;
/// }
/// ```
///
/// This would represent the `trait Foo` declaration. Note that the details of
/// the trait members (e.g., the associated type declaration (`type Bar<U>`) are
/// not contained in this type, and are represented separately (e.g., in
/// [`AssociatedTyDatum`]).
///
/// Not to be confused with the rust_ir for a Trait Implementation, which is
/// represented by [`ImplDatum`]
///
/// [`ImplDatum`]: struct.ImplDatum.html
/// [`AssociatedTyDatum`]: struct.AssociatedTyDatum.html
pub struct TraitDatum<I: Interner> {
    pub id: TraitId<I>,

    pub binders: Binders<TraitDatumBound<I>>,

    /// "Flags" indicate special kinds of traits, like auto traits.
    /// In Rust syntax these are represented in different ways, but in
    /// chalk we add annotations like `#[auto]`.
    pub flags: TraitFlags,

    pub associated_ty_ids: Vec<AssocTypeId<I>>,

    /// If this is a well-known trait, which one? If `None`, this is a regular,
    /// user-defined trait.
    pub well_known: Option<WellKnownTrait>,
}

/// A list of the traits that are "well known" to chalk, which means that
/// the chalk-solve crate has special, hard-coded impls for them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub enum WellKnownTrait {
    SizedTrait,
    CopyTrait,
    CloneTrait,
}

impl<I: Interner> TraitDatum<I> {
    pub fn is_auto_trait(&self) -> bool {
        self.flags.auto
    }

    pub fn is_non_enumerable_trait(&self) -> bool {
        self.flags.non_enumerable
    }

    pub fn is_coinductive_trait(&self) -> bool {
        self.flags.coinductive
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TraitDatumBound<I: Interner> {
    /// Where clauses defined on the trait:
    ///
    /// ```ignore
    /// trait Foo<T> where T: Debug { }
    ///              ^^^^^^^^^^^^^^
    /// ```
    pub where_clauses: Vec<QuantifiedWhereClause<I>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TraitFlags {
    /// An "auto trait" is one that is "automatically implemented" for every
    /// struct, so long as no explicit impl is given.
    ///
    /// Examples are `Send` and `Sync`.
    pub auto: bool,

    pub marker: bool,

    /// Indicate that a trait is defined upstream (in a dependency), used during
    /// coherence checking.
    pub upstream: bool,

    /// A fundamental trait is a trait where adding an impl for an existing type
    /// is considered a breaking change. Examples of fundamental traits are the
    /// closure traits like `Fn` and `FnMut`.
    ///
    /// As of this writing (2020-03-27), fundamental traits are declared by the
    /// unstable `#[fundamental]` attribute in rustc, and hence cannot appear
    /// outside of the standard library.
    pub fundamental: bool,

    /// Indicates that chalk cannot list all of the implementations of the given
    /// trait, likely because it is a publicly exported trait in a library.
    ///
    /// Currently (2020-03-27) rustc and rust-analyzer mark all traits as
    /// non_enumerable, and in the future it may become the only option.
    pub non_enumerable: bool,

    pub coinductive: bool,
}

/// An inline bound, e.g. `: Foo<K>` in `impl<K, T: Foo<K>> SomeType<T>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Fold, HasInterner)]
pub enum InlineBound<I: Interner> {
    TraitBound(TraitBound<I>),
    AliasEqBound(AliasEqBound<I>),
}

#[allow(type_alias_bounds)]
pub type QuantifiedInlineBound<I: Interner> = Binders<InlineBound<I>>;

pub trait IntoWhereClauses<I: Interner> {
    type Output;

    fn into_where_clauses(&self, interner: &I, self_ty: Ty<I>) -> Vec<Self::Output>;
}

impl<I: Interner> IntoWhereClauses<I> for InlineBound<I> {
    type Output = WhereClause<I>;

    /// Applies the `InlineBound` to `self_ty` and lowers to a
    /// [`chalk_ir::DomainGoal`].
    ///
    /// Because an `InlineBound` does not know anything about what it's binding,
    /// you must provide that type as `self_ty`.
    fn into_where_clauses(&self, interner: &I, self_ty: Ty<I>) -> Vec<WhereClause<I>> {
        match self {
            InlineBound::TraitBound(b) => b.into_where_clauses(interner, self_ty),
            InlineBound::AliasEqBound(b) => b.into_where_clauses(interner, self_ty),
        }
    }
}

impl<I: Interner> IntoWhereClauses<I> for QuantifiedInlineBound<I> {
    type Output = QuantifiedWhereClause<I>;

    fn into_where_clauses(&self, interner: &I, self_ty: Ty<I>) -> Vec<QuantifiedWhereClause<I>> {
        let self_ty = self_ty.shifted_in(interner);
        self.value
            .into_where_clauses(interner, self_ty)
            .into_iter()
            .map(|wc| Binders {
                binders: self.binders.clone(),
                value: wc,
            })
            .collect()
    }
}

/// Represents a trait bound on e.g. a type or type parameter.
/// Does not know anything about what it's binding.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Fold)]
pub struct TraitBound<I: Interner> {
    pub trait_id: TraitId<I>,
    pub args_no_self: Vec<Parameter<I>>,
}

impl<I: Interner> TraitBound<I> {
    fn into_where_clauses(&self, interner: &I, self_ty: Ty<I>) -> Vec<WhereClause<I>> {
        let trait_ref = self.as_trait_ref(interner, self_ty);
        vec![WhereClause::Implemented(trait_ref)]
    }

    pub fn as_trait_ref(&self, interner: &I, self_ty: Ty<I>) -> TraitRef<I> {
        TraitRef {
            trait_id: self.trait_id,
            substitution: Substitution::from(
                interner,
                iter::once(self_ty.cast(interner)).chain(self.args_no_self.iter().cloned()),
            ),
        }
    }
}

/// Represents an alias equality bound on e.g. a type or type parameter.
/// Does not know anything about what it's binding.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Fold)]
pub struct AliasEqBound<I: Interner> {
    pub trait_bound: TraitBound<I>,
    pub associated_ty_id: AssocTypeId<I>,
    /// Does not include trait parameters.
    pub parameters: Vec<Parameter<I>>,
    pub value: Ty<I>,
}

impl<I: Interner> AliasEqBound<I> {
    fn into_where_clauses(&self, interner: &I, self_ty: Ty<I>) -> Vec<WhereClause<I>> {
        let trait_ref = self.trait_bound.as_trait_ref(interner, self_ty);

        let substitution = Substitution::from(
            interner,
            self.parameters
                .iter()
                .cloned()
                .chain(trait_ref.substitution.iter(interner).cloned()),
        );

        vec![
            WhereClause::Implemented(trait_ref),
            WhereClause::AliasEq(AliasEq {
                alias: AliasTy {
                    associated_ty_id: self.associated_ty_id,
                    substitution,
                },
                ty: self.value.clone(),
            }),
        ]
    }
}

pub trait Anonymize {
    /// Utility function that converts from a list of generic parameters
    /// which *have* names (`ParameterKind<T>`) to a list of
    /// "anonymous" generic parameters that just preserves their
    /// kinds (`ParameterKind<()>`). Often convenient in lowering.
    fn anonymize(&self) -> Vec<ParameterKind<()>>;
}

impl<T> Anonymize for [ParameterKind<T>] {
    fn anonymize(&self) -> Vec<ParameterKind<()>> {
        self.iter().map(|pk| pk.map_ref(|_| ())).collect()
    }
}

pub trait ToParameter {
    /// Utility for converting a list of all the binders into scope
    /// into references to those binders. Simply pair the binders with
    /// the indices, and invoke `to_parameter()` on the `(binder,
    /// index)` pair. The result will be a reference to a bound
    /// variable of appropriate kind at the corresponding index.
    fn to_parameter<I: Interner>(&self, interner: &I) -> Parameter<I> {
        self.to_parameter_at_depth(interner, DebruijnIndex::INNERMOST)
    }

    fn to_parameter_at_depth<I: Interner>(
        &self,
        interner: &I,
        debruijn: DebruijnIndex,
    ) -> Parameter<I>;
}

impl<'a> ToParameter for (&'a ParameterKind<()>, usize) {
    fn to_parameter_at_depth<I: Interner>(
        &self,
        interner: &I,
        debruijn: DebruijnIndex,
    ) -> Parameter<I> {
        let &(binder, index) = self;
        let bound_var = BoundVar::new(debruijn, index);
        match *binder {
            ParameterKind::Lifetime(_) => LifetimeData::BoundVar(bound_var)
                .intern(interner)
                .cast(interner),
            ParameterKind::Ty(_) => TyData::BoundVar(bound_var).intern(interner).cast(interner),
        }
    }
}

/// Represents an associated type declaration found inside of a trait:
///
/// ```notrust
/// trait Foo<P1..Pn> { // P0 is Self
///     type Bar<Pn..Pm>: [bounds]
///     where
///         [where_clauses];
/// }
/// ```
///
/// The meaning of each of these parts:
///
/// * The *parameters* `P0...Pm` are all in scope for this associated type.
/// * The *bounds* `bounds` are things that the impl must prove to be true.
/// * The *where clauses* `where_clauses` are things that the impl can *assume* to be true
///   (but which projectors must prove).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssociatedTyDatum<I: Interner> {
    /// The trait this associated type is defined in.
    pub trait_id: TraitId<I>,

    /// The ID of this associated type
    pub id: AssocTypeId<I>,

    /// Name of this associated type.
    pub name: I::Identifier,

    /// These binders represent the `P0...Pm` variables.  The binders
    /// are in the order `[Pn..Pm; P0..Pn]`. That is, the variables
    /// from `Bar` come first (corresponding to the de bruijn concept
    /// that "inner" binders are lower indices, although within a
    /// given binder we do not have an ordering).
    pub binders: Binders<AssociatedTyDatumBound<I>>,
}

/// Encodes the parts of `AssociatedTyDatum` where the parameters
/// `P0..Pm` are in scope (`bounds` and `where_clauses`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Fold, HasInterner)]
pub struct AssociatedTyDatumBound<I: Interner> {
    /// Bounds on the associated type itself.
    ///
    /// These must be proven by the implementer, for all possible parameters that
    /// would result in a well-formed projection.
    pub bounds: Vec<QuantifiedInlineBound<I>>,

    /// Where clauses that must hold for the projection to be well-formed.
    pub where_clauses: Vec<QuantifiedWhereClause<I>>,
}

impl<I: Interner> AssociatedTyDatum<I> {
    /// Returns the associated ty's bounds applied to the projection type, e.g.:
    ///
    /// ```notrust
    /// Implemented(<?0 as Foo>::Item<?1>: Sized)
    /// ```
    ///
    /// these quantified where clauses are in the scope of the
    /// `binders` field.
    pub fn bounds_on_self(&self, interner: &I) -> Vec<QuantifiedWhereClause<I>> {
        let Binders { binders, value } = &self.binders;

        // Create a list `P0...Pn` of references to the binders in
        // scope for this associated type:
        let substitution = Substitution::from(
            interner,
            binders.iter().zip(0..).map(|p| p.to_parameter(interner)),
        );

        // The self type will be `<P0 as Foo<P1..Pn>>::Item<Pn..Pm>` etc
        let self_ty = TyData::Alias(AliasTy {
            associated_ty_id: self.id,
            substitution,
        })
        .intern(interner);

        // Now use that as the self type for the bounds, transforming
        // something like `type Bar<Pn..Pm>: Debug` into
        //
        // ```
        // <P0 as Foo<P1..Pn>>::Item<Pn..Pm>: Debug
        // ```
        value
            .bounds
            .iter()
            .flat_map(|b| b.into_where_clauses(interner, self_ty.clone()))
            .collect()
    }
}

/// Represents the *value* of an associated type that is assigned
/// from within some impl.
///
/// ```ignore
/// impl Iterator for Foo {
///     type Item = XXX; // <-- represents this line!
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, Fold)]
pub struct AssociatedTyValue<I: Interner> {
    /// Impl in which this associated type value is found.  You might
    /// need to look at this to find the generic parameters defined on
    /// the impl, for example.
    ///
    /// ```ignore
    /// impl Iterator for Foo { // <-- refers to this impl
    ///     type Item = XXX; // <-- (where this is self)
    /// }
    /// ```
    pub impl_id: ImplId<I>,

    /// Associated type being defined.
    ///
    /// ```ignore
    /// impl Iterator for Foo {
    ///     type Item = XXX; // <-- (where this is self)
    /// }
    /// ...
    /// trait Iterator {
    ///     type Item; // <-- refers to this declaration here!
    /// }
    /// ```
    pub associated_ty_id: AssocTypeId<I>,

    /// Additional binders declared on the associated type itself,
    /// beyond those from the impl. This would be empty for normal
    /// associated types, but non-empty for generic associated types.
    ///
    /// ```ignore
    /// impl<T> Iterable for Vec<T> {
    ///     type Iter<'a> = vec::Iter<'a, T>;
    ///           // ^^^^ refers to these generics here
    /// }
    /// ```
    pub value: Binders<AssociatedTyValueBound<I>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Fold, HasInterner)]
pub struct AssociatedTyValueBound<I: Interner> {
    /// Type that we normalize to. The X in `type Foo<'a> = X`.
    pub ty: Ty<I>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Polarity {
    Positive,
    Negative,
}

impl Polarity {
    pub fn is_positive(&self) -> bool {
        match *self {
            Polarity::Positive => true,
            Polarity::Negative => false,
        }
    }
}

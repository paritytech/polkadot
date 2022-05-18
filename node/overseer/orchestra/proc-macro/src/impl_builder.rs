// Copyright 2021 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use quote::{format_ident, quote};
use syn::{parse_quote, Path, PathSegment};

use super::*;

fn recollect_without_idx<T: Clone>(x: &[T], idx: usize) -> Vec<T> {
	let mut v = Vec::<T>::with_capacity(x.len().saturating_sub(1));
	v.extend(x.iter().take(idx).cloned());
	v.extend(x.iter().skip(idx + 1).cloned());
	v
}

/// Implement a builder pattern for the `Orchestra`-type,
/// which acts as the gateway to constructing the overseer.
///
/// Elements tagged with `wip` are not covered here.
pub(crate) fn impl_builder(info: &OrchestraInfo) -> proc_macro2::TokenStream {
	let overseer_name = info.overseer_name.clone();
	let builder = format_ident!("{}Builder", overseer_name);
	let handle = format_ident!("{}Handle", overseer_name);
	let connector = format_ident!("{}Connector", overseer_name);
	let subsystem_ctx_name = format_ident!("{}SubsystemContext", overseer_name);

	let subsystem_name = &info.subsystem_names_without_wip();
	let subsystem_generics = &info.subsystem_generic_types();

	let consumes = &info.consumes_without_wip();
	let channel_name = &info.channel_names_without_wip("");
	let channel_name_unbounded = &info.channel_names_without_wip("_unbounded");

	let channel_name_tx = &info.channel_names_without_wip("_tx");
	let channel_name_unbounded_tx = &info.channel_names_without_wip("_unbounded_tx");

	let channel_name_rx = &info.channel_names_without_wip("_rx");
	let channel_name_unbounded_rx = &info.channel_names_without_wip("_unbounded_rx");

	let baggage_name = &info.baggage_names();
	let baggage_generic_ty = &info.baggage_generic_types();

	// State generics that are used to encode each field's status (Init/Missing)
	let baggage_passthrough_state_generics = baggage_name
		.iter()
		.enumerate()
		.map(|(idx, _)| format_ident!("InitStateBaggage{}", idx))
		.collect::<Vec<_>>();
	let subsystem_passthrough_state_generics = subsystem_name
		.iter()
		.enumerate()
		.map(|(idx, _)| format_ident!("InitStateSubsystem{}", idx))
		.collect::<Vec<_>>();

	let error_ty = &info.extern_error_ty;

	let support_crate = info.support_crate_name();

	let blocking = &info
		.subsystems()
		.iter()
		.map(|x| {
			if x.blocking {
				quote! { Blocking }
			} else {
				quote! { Regular }
			}
		})
		.collect::<Vec<_>>();

	// Helpers to use within quote! macros
	let spawner_where_clause: syn::TypeParam = parse_quote! {
			S: #support_crate ::Spawner
	};

	// Field names and real types
	let field_name = subsystem_name.iter().chain(baggage_name.iter()).collect::<Vec<_>>();
	let field_type = subsystem_generics
		.iter()
		.map(|ident| Path::from(PathSegment::from(ident.clone())))
		.chain(info.baggage().iter().map(|bag| bag.field_ty.clone()))
		.collect::<Vec<_>>();

	// Setters logic

	// For each setter we need to leave the remaining fields untouched and
	// remove the field that we are fixing in this setter
	// For subsystems `*_with` and `replace_*` setters are needed.
	let subsystem_specific_setters =
		info.subsystems().iter().filter(|ssf| !ssf.wip).enumerate().map(|(idx, ssf)| {
			let field_name = &ssf.name;
			let field_type = &ssf.generic;
			let subsystem_consumes = &ssf.message_to_consume;
			// Remove state generic for the item to be replaced. It sufficient to know `field_type` for
			// that since we always move from `Init<#field_type>` to `Init<NEW>`.
			let impl_subsystem_state_generics = recollect_without_idx(&subsystem_passthrough_state_generics[..], idx);

			let field_name_with = format_ident!("{}_with", field_name);
			let field_name_replace = format_ident!("replace_{}", field_name);

			// In a setter we replace `Uninit<T>` with `Init<T>` leaving all other
			// types as they are, as such they will be free generics.
			let mut current_state_generics = subsystem_passthrough_state_generics
				.iter()
				.map(|subsystem_state_generic_ty| parse_quote!(#subsystem_state_generic_ty))
				.collect::<Vec<syn::GenericArgument>>();
			current_state_generics[idx] = parse_quote! { Missing<#field_type> };

			// Generics that will be present after initializing a specific `Missing<_>` field.
			let mut post_setter_state_generics = current_state_generics.clone();
			post_setter_state_generics[idx] = parse_quote! { Init<#field_type> };

			let mut post_replace_state_generics = current_state_generics.clone();
			post_replace_state_generics[idx] = parse_quote! { Init<NEW> };

			// All fields except the one we update with the new argument
			// see the loop below.
			let to_keep_subsystem_name = recollect_without_idx(&subsystem_name[..], idx);

			let subsystem_sender_trait = format_ident!("{}SenderTrait", field_type);
			let _subsystem_ctx_trait = format_ident!("{}ContextTrait", field_type);

			let builder_where_clause = quote!{
				#field_type : #support_crate::Subsystem< #subsystem_ctx_name< #subsystem_consumes >, #error_ty>,
				< #subsystem_ctx_name < #subsystem_consumes > as #support_crate :: SubsystemContext>::Sender:
					#subsystem_sender_trait,
			};

			// Create the field init `fn`
			quote! {
				impl <InitStateSpawner, #field_type, #( #impl_subsystem_state_generics, )* #( #baggage_passthrough_state_generics, )*>
				#builder <InitStateSpawner, #( #current_state_generics, )* #( #baggage_passthrough_state_generics, )*>
				where
					#builder_where_clause
				{
					/// Specify the subsystem in the builder directly
					pub fn #field_name (self, var: #field_type ) ->
						#builder <InitStateSpawner, #( #post_setter_state_generics, )* #( #baggage_passthrough_state_generics, )*>
					{
						#builder {
							#field_name: Init::< #field_type >::Value(var),
							#(
								#to_keep_subsystem_name: self. #to_keep_subsystem_name,
							)*
							#(
								#baggage_name: self. #baggage_name,
							)*
							spawner: self.spawner,

							channel_capacity: self.channel_capacity,
							signal_capacity: self.signal_capacity,
						}
					}
					/// Specify the the initialization function for a subsystem
					pub fn #field_name_with<'a, F>(self, subsystem_init_fn: F ) ->
						#builder <InitStateSpawner, #( #post_setter_state_generics, )* #( #baggage_passthrough_state_generics, )*>
					where
						F: 'static + FnOnce(#handle) ->
							::std::result::Result<#field_type, #error_ty>,
					{
						let boxed_func = Init::<#field_type>::Fn(
							Box::new(subsystem_init_fn) as SubsystemInitFn<#field_type>
						);
						#builder {
							#field_name: boxed_func,
							#(
								#to_keep_subsystem_name: self. #to_keep_subsystem_name,
							)*
							#(
								#baggage_name: self. #baggage_name,
							)*
							spawner: self.spawner,


							channel_capacity: self.channel_capacity,
							signal_capacity: self.signal_capacity,
						}
					}
				}

				impl <InitStateSpawner, #field_type, #( #impl_subsystem_state_generics, )* #( #baggage_passthrough_state_generics, )*>
				#builder <InitStateSpawner, #( #post_setter_state_generics, )* #( #baggage_passthrough_state_generics, )*>
				where
					#builder_where_clause
				{
					/// Replace a subsystem by another implementation for the
					/// consumable message type.
					pub fn #field_name_replace<NEW, F>(self, gen_replacement_fn: F)
						-> #builder <InitStateSpawner, #( #post_replace_state_generics, )* #( #baggage_passthrough_state_generics, )*>
					where
						#field_type: 'static,
						F: 'static + FnOnce(#field_type) -> NEW,
						NEW: #support_crate ::Subsystem<#subsystem_ctx_name< #subsystem_consumes >, #error_ty>,
					{
						let replacement: Init<NEW> = match self.#field_name {
							Init::Fn(fx) =>
								Init::<NEW>::Fn(Box::new(move |handle: #handle| {
								let orig = fx(handle)?;
								Ok(gen_replacement_fn(orig))
							})),
							Init::Value(val) =>
								Init::Value(gen_replacement_fn(val)),
						};
						#builder {
							#field_name: replacement,
							#(
								#to_keep_subsystem_name: self. #to_keep_subsystem_name,
							)*
							#(
								#baggage_name: self. #baggage_name,
							)*
							spawner: self.spawner,

							channel_capacity: self.channel_capacity,
							signal_capacity: self.signal_capacity,
						}
					}
				}
			}
		});

	// Produce setters for all baggage fields as well
	let baggage_specific_setters = info.baggage().iter().enumerate().map(|(idx, bag_field)| {
		// Baggage fields follow subsystems
		let fname = &bag_field.field_name;
		let field_type = &bag_field.field_ty;
		let impl_baggage_state_generics = recollect_without_idx(&baggage_passthrough_state_generics[..], idx);
		let to_keep_baggage_name = recollect_without_idx(&baggage_name[..], idx);

		let mut pre_setter_generics = baggage_passthrough_state_generics
			.iter()
			.map(|gen_ty| parse_quote!(#gen_ty))
			.collect::<Vec<syn::GenericArgument>>();
		pre_setter_generics[idx] = parse_quote! { Missing<#field_type> };

		let mut post_setter_generics = pre_setter_generics.clone();
		post_setter_generics[idx] = parse_quote! { Init<#field_type> };

		// Baggage can also be generic, so we need to include that to a signature
		let preserved_baggage_generic = if bag_field.generic {
			quote! {#field_type,}
		} else {
			TokenStream::new()
		};

		quote! {
			impl <InitStateSpawner, #preserved_baggage_generic #( #subsystem_passthrough_state_generics, )* #( #impl_baggage_state_generics, )* >
			#builder <InitStateSpawner, #( #subsystem_passthrough_state_generics, )* #( #pre_setter_generics, )* >
			{
				/// Specify the baggage in the builder when it was not initialized before
				pub fn #fname (self, var: #field_type ) ->
					#builder <InitStateSpawner, #( #subsystem_passthrough_state_generics, )* #( #post_setter_generics, )* >
				{
					#builder {
						#fname: Init::<#field_type>::Value(var),
						#(
							#subsystem_name: self. #subsystem_name,
						)*
						#(
							#to_keep_baggage_name: self. #to_keep_baggage_name,
						)*
						spawner: self.spawner,

						channel_capacity: self.channel_capacity,
						signal_capacity: self.signal_capacity,
					}
				}
			}
			impl <InitStateSpawner, #preserved_baggage_generic #( #subsystem_passthrough_state_generics, )* #( #impl_baggage_state_generics, )* >
			#builder <InitStateSpawner, #( #subsystem_passthrough_state_generics, )* #( #post_setter_generics, )* > {
				/// Specify the baggage in the builder when it has been previously initialized
				pub fn #fname (self, var: #field_type ) ->
					#builder <InitStateSpawner, #( #subsystem_passthrough_state_generics, )* #( #post_setter_generics, )* >
				{
					#builder {
						#fname: Init::<#field_type>::Value(var),
						#(
							#subsystem_name: self. #subsystem_name,
						)*
						#(
							#to_keep_baggage_name: self. #to_keep_baggage_name,
						)*
						spawner: self.spawner,

						channel_capacity: self.channel_capacity,
						signal_capacity: self.signal_capacity,
					}
				}
			}
		}
	});

	let event = &info.extern_event_ty;
	let initialized_builder = format_ident!("Initialized{}", builder);
	// The direct generics as expected by the `Orchestra<_,_,..>`, without states
	let initialized_builder_generics = quote! {
		S, #( #baggage_generic_ty, )* #( #subsystem_generics, )*
	};

	let builder_where_clause = info
		.subsystems()
		.iter()
		.map(|ssf| {
			let field_type = &ssf.generic;
			let consumes = &ssf.message_to_consume;
			let subsystem_sender_trait = format_ident!("{}SenderTrait", ssf.generic);
			let subsystem_ctx_trait = format_ident!("{}ContextTrait", ssf.generic);
			quote! {
				#field_type:
					#support_crate::Subsystem< #subsystem_ctx_name < #consumes>, #error_ty>,
				<#subsystem_ctx_name< #consumes > as #subsystem_ctx_trait>::Sender:
					#subsystem_sender_trait,
				#subsystem_ctx_name< #consumes >:
					#subsystem_ctx_trait,
			}
		})
		.fold(TokenStream::new(), |mut ts, addendum| {
			ts.extend(addendum);
			ts
		});

	let mut ts = quote! {
		/// Convenience alias.
		type SubsystemInitFn<T> = Box<dyn FnOnce(#handle) -> ::std::result::Result<T, #error_ty> >;

		/// Type for the initialized field of the overseer builder
		pub enum Init<T> {
			/// Defer initialization to a point where the `handle` is available.
			Fn(SubsystemInitFn<T>),
			/// Directly initialize the subsystem with the given subsystem type `T`.
			/// Also used for baggage fields
			Value(T),
		}
		/// Type marker for the uninitialized field of the overseer builder.
		/// `PhantomData` is used for type hinting when creating uninitialized
		/// builder, e.g. to avoid specifying the generics when instantiating
		/// the `FooBuilder` when calling `Foo::builder()`
		#[derive(Debug)]
		pub struct Missing<T>(::core::marker::PhantomData<T>);

		/// Trait used to mark fields status in a builder
		trait OrchestraFieldState<T> {}

		impl<T> OrchestraFieldState<T> for Init<T> {}
		impl<T> OrchestraFieldState<T> for Missing<T> {}

		impl<T> ::std::default::Default for Missing<T> {
			fn default() -> Self {
				Missing::<T>(::core::marker::PhantomData::<T>::default())
			}
		}

		impl<S #(, #baggage_generic_ty )*> #overseer_name <S #(, #baggage_generic_ty)*>
		where
			#spawner_where_clause,
		{
			/// Create a new overseer utilizing the builder.
			pub fn builder< #( #subsystem_generics),* >() ->
				#builder<Missing<S> #(, Missing< #field_type > )* >
			where
				#builder_where_clause
			{
				#builder :: new()
			}
		}
	};

	ts.extend(quote! {
		/// Handle for an overseer.
		pub type #handle = #support_crate ::metered::MeteredSender< #event >;

		/// External connector.
		pub struct #connector {
			/// Publicly accessible handle, to be used for setting up
			/// components that are _not_ subsystems but access is needed
			/// due to other limitations.
			///
			/// For subsystems, use the `_with` variants of the builder.
			handle: #handle,
			/// The side consumed by the `spawned` side of the overseer pattern.
			consumer: #support_crate ::metered::MeteredReceiver < #event >,
		}

		impl #connector {
			/// Obtain access to the overseer handle.
			pub fn as_handle_mut(&mut self) -> &mut #handle {
				&mut self.handle
			}
			/// Obtain access to the overseer handle.
			pub fn as_handle(&self) -> &#handle {
				&self.handle
			}
			/// Obtain a clone of the handle.
			pub fn handle(&self) -> #handle {
				self.handle.clone()
			}

			/// Create a new connector with non-default event channel capacity.
			pub fn with_event_capacity(event_channel_capacity: usize) -> Self {
				let (events_tx, events_rx) = #support_crate ::metered::channel::<
					#event
					>(event_channel_capacity);

				Self {
					handle: events_tx,
					consumer: events_rx,
				}
			}
		}

		impl ::std::default::Default for #connector {
			fn default() -> Self {
				Self::with_event_capacity(SIGNAL_CHANNEL_CAPACITY)
			}
		}
	});

	ts.extend(quote!{
		/// Builder pattern to create compile time safe construction path.
		pub struct #builder <InitStateSpawner, #( #subsystem_passthrough_state_generics, )* #( #baggage_passthrough_state_generics, )*>
		{
			#(
				#subsystem_name: #subsystem_passthrough_state_generics,
			)*
			#(
				#baggage_name: #baggage_passthrough_state_generics,
			)*
			spawner: InitStateSpawner,
			// user provided runtime overrides,
			// if `None`, the `orchestra(message_capacity=123,..)` is used
			// or the default value.
			channel_capacity: Option<usize>,
			signal_capacity: Option<usize>,
		}
	});

	ts.extend(quote! {
		impl<#initialized_builder_generics> #builder<Missing<S>, #( Missing<#field_type>, )*>
		{
			/// Create a new builder pattern, with all fields being uninitialized.
			fn new() -> Self {
				// explicitly assure the required traits are implemented
				fn trait_from_must_be_implemented<E>()
				where
					E: std::error::Error + Send + Sync + 'static + From<#support_crate ::OrchestraError>
				{}

				trait_from_must_be_implemented::< #error_ty >();

				Self {
					#(
						#field_name: Missing::<#field_type>::default(),
					)*
					spawner: Missing::<S>::default(),

					channel_capacity: None,
					signal_capacity: None,
				}
			}
		}
	});

	// Spawner setter
	ts.extend(quote!{
		impl<S, #( #subsystem_passthrough_state_generics, )* #( #baggage_passthrough_state_generics, )*>
			#builder<Missing<S>, #( #subsystem_passthrough_state_generics, )* #( #baggage_passthrough_state_generics, )*>
		where
			#spawner_where_clause,
		{
			/// The `spawner` to use for spawning tasks.
			pub fn spawner(self, spawner: S) -> #builder<
				Init<S>,
				#( #subsystem_passthrough_state_generics, )*
				#( #baggage_passthrough_state_generics, )*
			>
			{
				#builder {
					#(
						#field_name: self. #field_name,
					)*
					spawner: Init::<S>::Value(spawner),

					channel_capacity: self.channel_capacity,
					signal_capacity: self.signal_capacity,
				}
			}
		}
	});

	// message and signal channel capacity
	ts.extend(quote! {
		impl<S, #( #subsystem_passthrough_state_generics, )* #( #baggage_passthrough_state_generics, )*>
			#builder<Init<S>, #( #subsystem_passthrough_state_generics, )* #( #baggage_passthrough_state_generics, )*>
		where
			#spawner_where_clause,
		{
			/// Set the interconnecting signal channel capacity.
			pub fn signal_channel_capacity(mut self, capacity: usize) -> Self
			{
				self.signal_capacity = Some(capacity);
				self
			}

			/// Set the interconnecting message channel capacities.
			pub fn message_channel_capacity(mut self, capacity: usize) -> Self
			{
				self.channel_capacity = Some(capacity);
				self
			}
		}
	});

	// Create the string literals for spawn.
	let subsystem_name_str_literal = subsystem_name
		.iter()
		.map(|ident| proc_macro2::Literal::string(ident.to_string().replace("_", "-").as_str()))
		.collect::<Vec<_>>();

	ts.extend(quote! {
		/// Type used to represent a builder where all fields are initialized and the overseer could be constructed.
		pub type #initialized_builder<#initialized_builder_generics> = #builder<Init<S>, #( Init<#field_type>, )*>;

		// A builder specialization where all fields are set
		impl<#initialized_builder_generics> #initialized_builder<#initialized_builder_generics>
		where
			#spawner_where_clause,
			#builder_where_clause
		{
			/// Complete the construction and create the overseer type.
			pub fn build(self)
				-> ::std::result::Result<(#overseer_name<S, #( #baggage_generic_ty, )*>, #handle), #error_ty> {
				let connector = #connector ::with_event_capacity(
					self.signal_capacity.unwrap_or(SIGNAL_CHANNEL_CAPACITY)
				);
				self.build_with_connector(connector)
			}

			/// Complete the construction and create the overseer type based on an existing `connector`.
			pub fn build_with_connector(self, connector: #connector)
				-> ::std::result::Result<(#overseer_name<S, #( #baggage_generic_ty, )*>, #handle), #error_ty>
			{
				let #connector {
					handle: events_tx,
					consumer: events_rx,
				} = connector;

				let handle = events_tx.clone();

				let (to_overseer_tx, to_overseer_rx) = #support_crate ::metered::unbounded::<
					ToOrchestra
				>();

				#(
					let (#channel_name_tx, #channel_name_rx)
					=
						#support_crate ::metered::channel::<
							MessagePacket< #consumes >
						>(
							self.channel_capacity.unwrap_or(CHANNEL_CAPACITY)
						);
				)*

				#(
					let (#channel_name_unbounded_tx, #channel_name_unbounded_rx) =
						#support_crate ::metered::unbounded::<
							MessagePacket< #consumes >
						>();
				)*

				let channels_out =
					ChannelsOut {
						#(
							#channel_name: #channel_name_tx .clone(),
						)*
						#(
							#channel_name_unbounded: #channel_name_unbounded_tx,
						)*
					};

				let mut spawner = match self.spawner {
					Init::Value(value) => value,
					_ => unreachable!("Only ever init spawner as value. qed"),
				};

				let mut running_subsystems = #support_crate ::FuturesUnordered::<
						BoxFuture<'static, ::std::result::Result<(), #error_ty > >
					>::new();

				#(
					let #subsystem_name = match self. #subsystem_name {
						Init::Fn(func) => func(handle.clone())?,
						Init::Value(val) => val,
					};

					let unbounded_meter = #channel_name_unbounded_rx.meter().clone();

					let message_rx: SubsystemIncomingMessages< #consumes > = #support_crate ::select(
						#channel_name_rx, #channel_name_unbounded_rx
					);
					let (signal_tx, signal_rx) = #support_crate ::metered::channel(
						self.signal_capacity.unwrap_or(SIGNAL_CHANNEL_CAPACITY)
					);

					let ctx = #subsystem_ctx_name::< #consumes >::new(
						signal_rx,
						message_rx,
						channels_out.clone(),
						to_overseer_tx.clone(),
						#subsystem_name_str_literal
					);

					let #subsystem_name: OrchestratedSubsystem< #consumes > =
						spawn::<_,_, #blocking, _, _, _>(
							&mut spawner,
							#channel_name_tx,
							signal_tx,
							unbounded_meter,
							ctx,
							#subsystem_name,
							#subsystem_name_str_literal,
							&mut running_subsystems,
						)?;
				)*

				use #support_crate ::StreamExt;

				let to_overseer_rx = to_overseer_rx.fuse();
				let overseer = #overseer_name {
					#(
						#subsystem_name,
					)*

					#(
						#baggage_name: match self. #baggage_name {
							Init::Value(val) => val,
							_ => panic!("unexpected baggage initialization, must be value"),
						},
					)*

					spawner,
					running_subsystems,
					events_rx,
					to_overseer_rx,
				};

				Ok((overseer, handle))
			}
		}
	});

	ts.extend(baggage_specific_setters);
	ts.extend(subsystem_specific_setters);
	ts.extend(impl_task_kind(info));
	ts
}

pub(crate) fn impl_task_kind(info: &OrchestraInfo) -> proc_macro2::TokenStream {
	let signal = &info.extern_signal_ty;
	let error_ty = &info.extern_error_ty;
	let support_crate = info.support_crate_name();

	let ts = quote! {
		/// Task kind to launch.
		pub trait TaskKind {
			/// Spawn a task, it depends on the implementer if this is blocking or not.
			fn launch_task<S: Spawner>(spawner: &mut S, task_name: &'static str, subsystem_name: &'static str, future: BoxFuture<'static, ()>);
		}

		#[allow(missing_docs)]
		struct Regular;
		impl TaskKind for Regular {
			fn launch_task<S: Spawner>(spawner: &mut S, task_name: &'static str, subsystem_name: &'static str, future: BoxFuture<'static, ()>) {
				spawner.spawn(task_name, Some(subsystem_name), future)
			}
		}

		#[allow(missing_docs)]
		struct Blocking;
		impl TaskKind for Blocking {
			fn launch_task<S: Spawner>(spawner: &mut S, task_name: &'static str, subsystem_name: &'static str, future: BoxFuture<'static, ()>) {
				spawner.spawn_blocking(task_name, Some(subsystem_name), future)
			}
		}

		/// Spawn task of kind `self` using spawner `S`.
		pub fn spawn<S, M, TK, Ctx, E, SubSys>(
			spawner: &mut S,
			message_tx: #support_crate ::metered::MeteredSender<MessagePacket<M>>,
			signal_tx: #support_crate ::metered::MeteredSender< #signal >,
			// meter for the unbounded channel
			unbounded_meter: #support_crate ::metered::Meter,
			ctx: Ctx,
			s: SubSys,
			subsystem_name: &'static str,
			futures: &mut #support_crate ::FuturesUnordered<BoxFuture<'static, ::std::result::Result<(), #error_ty> >>,
		) -> ::std::result::Result<OrchestratedSubsystem<M>, #error_ty >
		where
			S: #support_crate ::Spawner,
			M: std::fmt::Debug + Send + 'static,
			TK: TaskKind,
			Ctx: #support_crate ::SubsystemContext<Message=M>,
			E: std::error::Error + Send + Sync + 'static + From<#support_crate ::OrchestraError>,
			SubSys: #support_crate ::Subsystem<Ctx, E>,
		{
			let #support_crate ::SpawnedSubsystem::<E> { future, name } = s.start(ctx);

			let (tx, rx) = #support_crate ::oneshot::channel();

			let fut = Box::pin(async move {
				if let Err(e) = future.await {
					#support_crate ::tracing::error!(subsystem=name, err = ?e, "subsystem exited with error");
				} else {
					#support_crate ::tracing::debug!(subsystem=name, "subsystem exited without an error");
				}
				let _ = tx.send(());
			});

			<TK as TaskKind>::launch_task(spawner, name, subsystem_name, fut);

			futures.push(Box::pin(
				rx.map(|e| {
					#support_crate ::tracing::warn!(err = ?e, "dropping error");
					Ok(())
				})
			));

			let instance = Some(SubsystemInstance {
				meters: #support_crate ::SubsystemMeters {
					unbounded: unbounded_meter,
					bounded: message_tx.meter().clone(),
					signals: signal_tx.meter().clone(),
				},
				tx_signal: signal_tx,
				tx_bounded: message_tx,
				signals_received: 0,
				name,
			});

			Ok(OrchestratedSubsystem {
				instance,
			})
		}
	};

	ts
}

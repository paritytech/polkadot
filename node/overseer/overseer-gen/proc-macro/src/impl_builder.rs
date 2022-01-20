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
use syn::Ident;
use inflector::Inflector;

use super::*;

/// Implement a builder pattern for the `Overseer`-type,
/// which acts as the gateway to constructing the overseer.
///
/// Elements tagged with `wip` are not covered here.
pub(crate) fn impl_builder(info: &OverseerInfo) -> proc_macro2::TokenStream {
	let overseer_name = info.overseer_name.clone();
	let builder = format_ident!("{}Builder", overseer_name);
	let handle = format_ident!("{}Handle", overseer_name);
	let connector = format_ident!("{}Connector", overseer_name);

	let subsystem_name = &info.subsystem_names_without_wip();

	let consumes = &info.consumes_without_wip();
	let channel_name = &info.channel_names_without_wip("");
	let channel_name_unbounded = &info.channel_names_without_wip("_unbounded");

	let channel_name_tx = &info.channel_names_without_wip("_tx");
	let channel_name_unbounded_tx = &info.channel_names_without_wip("_unbounded_tx");

	let channel_name_rx = &info.channel_names_without_wip("_rx");
	let channel_name_unbounded_rx = &info.channel_names_without_wip("_unbounded_rx");

	let baggage_name = &info.baggage_names();
	let subsystem_ctx_name = format_ident!("{}SubsystemContext", overseer_name);

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

	// A helper structure to carry fields of a builder to produce setters
	struct BuilderFieldHelper {
		// Used to distinguish baggage and subsystems
		is_subsystem: bool,
		// Valid for baggage as all subsystems are generics
		is_generic: bool,
		// Field name
		name: Ident,
		// Field type (either generic or not)
		field_ty: TokenStream,
		// Type to use in generics (like __FieldName)
		placeholder_type: TokenStream,
		// Used to constrain setter generics
		where_clause: Option<TokenStream>,
		// Specific initialised type, like OverseerFieldInit<T>
		inited_ty: TokenStream,
		// Specific uninitialised type, like OverseerFieldUninit<T>
		uninited_ty: TokenStream,
		// Order of the field in a target structure
		order: usize,
	}

	let mut field_helpers = info.subsystems()
		.iter()
		.filter(|ssf| !ssf.wip)
		.enumerate()
		.map(|(idx, ssf)| {
			let prefixed_ident = format_ident!("__{}",
				ssf.name.to_string().to_class_case());
			let consumes = &ssf.consumes;
			let generic_ty = &ssf.generic;
			BuilderFieldHelper {
				is_subsystem: true,
				is_generic: true,
				name: ssf.name.clone(),
				field_ty: quote!(#generic_ty),
				placeholder_type: quote! { #prefixed_ident },
				where_clause: Some(quote! {
					#generic_ty : #support_crate ::Subsystem<#subsystem_ctx_name< #consumes >, #error_ty>
				}),
				inited_ty: quote! {
					OverseerFieldInit<SubsystemFieldInitMethod<#generic_ty>>
				},
				uninited_ty: quote! {
					OverseerFieldUninit<SubsystemFieldInitMethod<#generic_ty>>
				},
				order: idx
			}
		})
		.collect::<Vec<_>>();
	let last_subsystem_idx = field_helpers.len();
	field_helpers.extend(info.baggage()
		.iter()
		.enumerate()
		.map(|(idx, bgf)| {
			let prefixed_ident = format_ident!("__{}", bgf.field_name
				.to_string().to_class_case());
			let baggage_ty = &bgf.field_ty;

			BuilderFieldHelper {
				is_subsystem: false,
				is_generic: bgf.generic,
				name: bgf.field_name.clone(),
				field_ty: quote!(#baggage_ty),
				placeholder_type: quote! { #prefixed_ident },
				where_clause: None,
				inited_ty: quote! {
					OverseerFieldInit<#baggage_ty>
				},
				uninited_ty: quote! {
					OverseerFieldUninit<#baggage_ty>
				},
				order: idx + last_subsystem_idx
			}
		})
	);

	let struct_common_generics = field_helpers
		.iter()
		.filter(|fh| fh.is_generic)
		.map(|fh| &fh.field_ty)
		.collect::<Vec<_>>();
	let builder_common_generics = field_helpers
		.iter()
		.map(|fh| &fh.placeholder_type)
		.collect::<Vec<_>>();
	let where_clause_elts = field_helpers
		.iter()
		.filter_map(|fh| fh.where_clause.as_ref())
		.collect::<Vec<_>>();

	let builder_where_clause = quote! {
		where
			#( #where_clause_elts, )*
			S: #support_crate ::SpawnNamed
	};

	let builder_subsystem_setters = field_helpers
		.iter()
		.enumerate()
		.map(|(idx, field_helper)| {
			let setter_name = &field_helper.name;
			let var_type = &field_helper.field_ty;

			let impl_generics = if !field_helper.is_generic {
				// For non generics we exclude type definition for the field being set
				field_helpers
					.iter()
					.filter(|other_field| other_field.order != idx)
					.map(|other_field| &other_field.placeholder_type)
					.collect::<Vec<_>>()
			}
			else {
				// For generics we replace placeholder type with a real generic type
				field_helpers
					.iter()
					.map(|other_field| {
						if other_field.order == idx {
							&other_field.field_ty
						}
						else {
							&other_field.placeholder_type
						}
					})
					.collect::<Vec<_>>()
			};
			// For the type generics we replace placeholder type with Uninit<T>
			let type_specific_generics = field_helpers
				.iter()
				.map(|other_field| {
					if other_field.order == idx {
						&other_field.uninited_ty
					}
					else {
						&other_field.placeholder_type
					}
				})
				.collect::<Vec<_>>();
			// For the setter return type generics we replace placeholder type with Init<T>
			let return_type_generics = field_helpers
				.iter()
				.map(|other_field| {
					if other_field.order == idx {
						&other_field.inited_ty
					}
					else {
						&other_field.placeholder_type
					}
				})
				.collect::<Vec<_>>();
			// To set specific fields in a setter
			let fields_definitions = field_helpers
				.iter()
				.map(|other_field| {
					let fname = &other_field.name;
					if other_field.order == idx {
						if other_field.is_subsystem {
							// We need to pack it one more time to distinguish from
							// Fn initialization
							quote! { #fname: OverseerFieldInit::
								<SubsystemFieldInitMethod::<#var_type>>(
									SubsystemFieldInitMethod::<#var_type>::Value(var)
								)
							}
						}
						else {
							quote! { #fname: OverseerFieldInit::<#var_type>(var) }
						}
					}
					else {
						quote!{ #fname: self.#fname }
					}
				})
				.collect::<Vec<_>>();
			// Setter that is common for baggage and subsystem
			let mut setter = quote! {
				impl <S, #( #impl_generics, )*>
				#builder <S, #( #type_specific_generics, )*> #builder_where_clause {
					/// Specify the field in the builder
					pub fn #setter_name (self, var: #var_type ) ->
						#builder <S, #( #return_type_generics, )*>
					{
						#builder {
							#(
								#fields_definitions,
							)*
							spawner: self.spawner,
						}
					}
				}
			};

			if field_helper.is_subsystem {
				// Produce one more setter to init from a function
				let setter_name_init_with = format_ident!("{}_with", setter_name);

				let other_fields_definitions = field_helpers
					.iter()
					.filter(|other_field| other_field.order != idx)
					.map(|other_field| {
						let fname = &other_field.name;
						quote!{ #fname: self.#fname }
					})
					.collect::<Vec<_>>();
				let setter_with = quote! {
					impl <S, #( #impl_generics, )*>
					#builder <S, #( #type_specific_generics, )*> #builder_where_clause {
						/// Specify the the init function for a subsystem
						pub fn #setter_name_init_with<'a, F>(self, subsystem_init_fn: F ) ->
							#builder <S, #( #return_type_generics, )*>
							where
							F: 'static + FnOnce(#handle) ->
								::std::result::Result<#var_type, #error_ty>,
						{
							let boxed_func = SubsystemFieldInitMethod::Fn(
								Box::new(subsystem_init_fn) as SubsystemInitFn<#var_type>
							);
							#builder {
								#(
									#other_fields_definitions,
								)*
								#setter_name: OverseerFieldInit::
									<SubsystemFieldInitMethod::<#var_type>>(boxed_func),
								spawner: self.spawner,
							}
						}
					}
				};
				setter.extend(setter_with);

				// For subsystems, we also need a replacement method
				let setter_name_replace = format_ident!("replace_{}", setter_name);
				let modified_generic_ty = quote!{
					OverseerFieldInit<SubsystemFieldInitMethod<NEW>>
				};
				let mut modified_generic_return_type = return_type_generics.clone();
				modified_generic_return_type[idx] = &modified_generic_ty;
				// We can use idx here as it is exactly the same as subsystems_wihout_wip idx
				let cur_consumes = &consumes[idx];
				let new_where_condition = quote! {
					#support_crate ::Subsystem<#subsystem_ctx_name< #cur_consumes >, #error_ty>
				};
				// Another note is that replace function requires the subsystem field
				// to be initialized first
				let setter_replace = quote! {
					impl <S, #( #impl_generics, )*>
					#builder <S, #( #return_type_generics, )*> #builder_where_clause {
						/// Replace a subsystem by another implementation for the
						/// consumable message type.
						pub fn #setter_name_replace<NEW, F>(self, gen_replacement_fn: F)
							-> #builder <S, #( #modified_generic_return_type, )*>
						where
							#var_type: 'static,
							F: 'static + FnOnce(#var_type) -> NEW,
							NEW: #new_where_condition,
						{
							let replacement: SubsystemFieldInitMethod<NEW> = match self.#setter_name.0 {
								SubsystemFieldInitMethod::Fn(fx) =>
									SubsystemFieldInitMethod::Fn(Box::new(move |handle: #handle| {
									let orig = fx(handle)?;
									Ok(gen_replacement_fn(orig))
								})),
								SubsystemFieldInitMethod::Value(val) =>
									SubsystemFieldInitMethod::Value(gen_replacement_fn(val)),
							};
							#builder::<S, #( #modified_generic_return_type, )*> {
								#setter_name: OverseerFieldInit::
									<SubsystemFieldInitMethod::<NEW>>(replacement),
								#(
									#other_fields_definitions,
								)*
								spawner: self.spawner,
							}
						}
					}
				};
				setter.extend(setter_replace);
			}
			setter
		})
		.collect::<Vec<_>>();

	// Used as a implementation spec for a builder with all fields uninitialised
	let all_uninit_generics = field_helpers
		.iter()
		.map(|fh| &fh.uninited_ty)
		.collect::<Vec<_>>();
	// Similarly, defines all field types as initialised
	let all_init_generics = field_helpers
		.iter()
		.map(|fh| &fh.inited_ty)
		.collect::<Vec<_>>();
	// Defines all fields to be set to uninit<T>
	let uninit_builder_field_def = field_helpers
		.iter()
		.map(|fh| {
			let (field_name, field_ty) = (&fh.name, &fh.field_ty);
			if fh.is_subsystem {
				quote! { #field_name: OverseerFieldUninit::<SubsystemFieldInitMethod<#field_ty>>::default() }
			}
			else {
				quote! { #field_name: OverseerFieldUninit::<#field_ty>::default() }
			}
		})
		.collect::<Vec<_>>();
	let struct_field_definition = field_helpers
		.iter()
		.map(|fh| {
			let (field_name, field_gen_ty) = (&fh.name, &fh.placeholder_type);
			quote!{ #field_name: #field_gen_ty }
		})
		.collect::<Vec<_>>();

	let builder_struct = quote! {
		pub struct #builder <S, #( #builder_common_generics, )*> {
			#(
				#struct_field_definition,
			)*
			spawner: ::std::option::Option< S >,
		}
	};

	let uninit_builder_impl = quote! {
		impl<S, #( #struct_common_generics, )*> #builder<S, #( #all_uninit_generics, )*>
		#builder_where_clause {
			fn new() -> Self {
				// explicitly assure the required traits are implemented
				fn trait_from_must_be_implemented<E>()
				where
					E: std::error::Error + Send + Sync + 'static + From<#support_crate ::OverseerError>
				{}

				trait_from_must_be_implemented::< #error_ty >();
				Self {
					#(
						#uninit_builder_field_def,
					)*
					spawner: None,
				}
			}
		}
	};

	let event = &info.extern_event_ty;

	let mut ts = quote! {
		/// Type for the initialised field of the overseer builder
		#[derive(Default, Debug)]
		pub struct OverseerFieldInit<T>(T);
		/// Type marker for the uninitialised field of the overseer builder
		/// Phantom data is used for type hinting when creating uninitialized
		/// builder.
		#[derive(Debug)]
		pub struct OverseerFieldUninit<T>(::core::marker::PhantomData<T>);

		/// Trait used to mark fields status in a builder
		trait OverseerFieldSt<T> {}

		impl<T> OverseerFieldSt<T> for OverseerFieldInit<T> {}
		impl<T> OverseerFieldSt<T> for OverseerFieldUninit<T> {}

		impl<T> std::default::Default for OverseerFieldUninit<T> {
			fn default() -> Self {
				OverseerFieldUninit::<T>(::core::marker::PhantomData::<T>::default())
			}
		}

		impl<S> #overseer_name <S> {
			/// Create a new overseer utilizing the builder.
			pub fn builder<#( #struct_common_generics, )*>() ->
				#builder<S, #( #all_uninit_generics, )* > #builder_where_clause
			{
				#builder :: new()
			}
		}

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
		}

		impl ::std::default::Default for #connector {
			fn default() -> Self {
				let (events_tx, events_rx) = #support_crate ::metered::channel::<
					#event
					>(SIGNAL_CHANNEL_CAPACITY);

				Self {
					handle: events_tx,
					consumer: events_rx,
				}
			}
		}

		/// Convenience alias.
		type SubsystemInitFn<T> = Box<dyn FnOnce(#handle) -> ::std::result::Result<T, #error_ty> >;

		/// Initialization type to be used for a field of the overseer.
		pub enum SubsystemFieldInitMethod<T> {
			/// Defer initialization to a point where the `handle` is available.
			Fn(SubsystemInitFn<T>),
			/// Directly initialize the subsystem with the given subsystem type `T`.
			Value(T),
		}

		#[allow(missing_docs)]
		#builder_struct

		#uninit_builder_impl

		#( #builder_subsystem_setters )*

		impl<S, #( #struct_common_generics, )*> #builder<S, #( #all_init_generics, )*>
		#builder_where_clause {
			/// The spawner to use for spawning tasks.
			pub fn spawner(mut self, spawner: S) -> Self
			where
				S: #support_crate ::SpawnNamed + Send
			{
				self.spawner = Some(spawner);
				self
			}

			/// Complete the construction and create the overseer type.
			pub fn build(self)
				-> ::std::result::Result<(#overseer_name<S>, #handle), #error_ty> {
				let connector = #connector ::default();
				self.build_with_connector(connector)
			}

			/// Complete the construction and create the overseer type based on an existing `connector`.
			pub fn build_with_connector(self, connector: #connector)
				-> ::std::result::Result<(#overseer_name<S>, #handle), #error_ty>
			{
				let #connector {
					handle: events_tx,
					consumer: events_rx,
				} = connector;

				let handle = events_tx.clone();

				let (to_overseer_tx, to_overseer_rx) = #support_crate ::metered::unbounded::<
					ToOverseer
				>();

				#(
					let (#channel_name_tx, #channel_name_rx)
					=
						#support_crate ::metered::channel::<
							MessagePacket< #consumes >
						>(CHANNEL_CAPACITY);
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

				let mut spawner = self.spawner.expect("Spawner is set. qed");

				let mut running_subsystems = #support_crate ::FuturesUnordered::<
						BoxFuture<'static, ::std::result::Result<(), #error_ty > >
					>::new();

				#(
					let #subsystem_name = match self. #subsystem_name .0 {
						SubsystemFieldInitMethod::Fn(func) => func(handle.clone())?,
						SubsystemFieldInitMethod::Value(val) => val,
					};

					let unbounded_meter = #channel_name_unbounded_rx.meter().clone();

					let message_rx: SubsystemIncomingMessages< #consumes > = #support_crate ::select(
						#channel_name_rx, #channel_name_unbounded_rx
					);
					let (signal_tx, signal_rx) = #support_crate ::metered::channel(SIGNAL_CHANNEL_CAPACITY);

					// Generate subsystem name based on overseer field name.
					let mut subsystem_string = String::from(stringify!(#subsystem_name));
					// Convert owned `snake case` string to a `kebab case` static str.
					let subsystem_static_str = Box::leak(subsystem_string.replace("_", "-").into_boxed_str());

					let ctx = #subsystem_ctx_name::< #consumes >::new(
						signal_rx,
						message_rx,
						channels_out.clone(),
						to_overseer_tx.clone(),
						subsystem_static_str
					);

					let #subsystem_name: OverseenSubsystem< #consumes > =
						spawn::<_,_, #blocking, _, _, _>(
							&mut spawner,
							#channel_name_tx,
							signal_tx,
							unbounded_meter,
							ctx,
							#subsystem_name,
							subsystem_static_str,
							&mut running_subsystems,
						)?;
				)*

				use #support_crate ::StreamExt;

				let to_overseer_rx = to_overseer_rx.fuse();
				let overseer = #overseer_name {
					#(
						#subsystem_name: #subsystem_name,
					)*

					#(
						#baggage_name: self.#baggage_name.0,
					)*

					spawner,
					running_subsystems,
					events_rx,
					to_overseer_rx,
				};

				Ok((overseer, handle))
			}
		}
	};

	ts.extend(impl_task_kind(info));
	ts
}

pub(crate) fn impl_task_kind(info: &OverseerInfo) -> proc_macro2::TokenStream {
	let signal = &info.extern_signal_ty;
	let error_ty = &info.extern_error_ty;
	let support_crate = info.support_crate_name();

	let ts = quote! {
		/// Task kind to launch.
		pub trait TaskKind {
			/// Spawn a task, it depends on the implementer if this is blocking or not.
			fn launch_task<S: SpawnNamed>(spawner: &mut S, task_name: &'static str, subsystem_name: &'static str, future: BoxFuture<'static, ()>);
		}

		#[allow(missing_docs)]
		struct Regular;
		impl TaskKind for Regular {
			fn launch_task<S: SpawnNamed>(spawner: &mut S, task_name: &'static str, subsystem_name: &'static str, future: BoxFuture<'static, ()>) {
				spawner.spawn(task_name, Some(subsystem_name), future)
			}
		}

		#[allow(missing_docs)]
		struct Blocking;
		impl TaskKind for Blocking {
			fn launch_task<S: SpawnNamed>(spawner: &mut S, task_name: &'static str, subsystem_name: &'static str, future: BoxFuture<'static, ()>) {
				spawner.spawn(task_name, Some(subsystem_name), future)
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
		) -> ::std::result::Result<OverseenSubsystem<M>, #error_ty >
		where
			S: #support_crate ::SpawnNamed,
			M: std::fmt::Debug + Send + 'static,
			TK: TaskKind,
			Ctx: #support_crate ::SubsystemContext<Message=M>,
			E: std::error::Error + Send + Sync + 'static + From<#support_crate ::OverseerError>,
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
					tracing::warn!(err = ?e, "dropping error");
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

			Ok(OverseenSubsystem {
				instance,
			})
		}
	};

	ts
}

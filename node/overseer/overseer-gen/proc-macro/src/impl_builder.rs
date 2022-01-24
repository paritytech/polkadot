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

use inflector::Inflector;
use quote::{format_ident, quote};
use syn::Ident;

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
	let subsystem_type = &info.subsystem_generic_types();

	let consumes = &info.consumes_without_wip();
	let channel_name = &info.channel_names_without_wip("");
	let channel_name_unbounded = &info.channel_names_without_wip("_unbounded");

	let channel_name_tx = &info.channel_names_without_wip("_tx");
	let channel_name_unbounded_tx = &info.channel_names_without_wip("_unbounded_tx");

	let channel_name_rx = &info.channel_names_without_wip("_rx");
	let channel_name_unbounded_rx = &info.channel_names_without_wip("_unbounded_rx");

	let baggage_name = &info.baggage_names();
	let baggage_generic = &info.baggage_generic_types();
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

	// Helpers to use within quote! macros
	let subsystem_where_clause = &info
		.subsystems()
		.iter()
		.map(|subsys| {
			let generic_ty = &subsys.generic;
			let consumes = &subsys.consumes;
			quote! {
				#generic_ty : #support_crate ::Subsystem<#subsystem_ctx_name< #consumes >, #error_ty>
			}
		})
		.collect::<Vec<_>>();
	let builder_where_clause = quote! {
		where
			#( #subsystem_where_clause, )*
			S: #support_crate ::SpawnNamed + Send
	};
	let builder_generics = quote! {
		S, #( #subsystem_type, )* #( #baggage_generic, )*
	};

	let placeholder_type =
		create_all_placeholder_idents(subsystem_name.as_slice(), baggage_name.as_slice());
	let field_name = subsystem_name.iter().chain(baggage_name.iter()).collect::<Vec<_>>();
	let field_type = subsystem_type
		.iter()
		.chain(info.baggage().iter().filter_map(|bag| bag.field_ty.get_ident()))
		.collect::<Vec<_>>();

	// Setters logic

	// For each setter we need to leave the remaining fields untouched and
	// remove the field that we are fixing in this setter
	// For subsystems we also need _with and replace_ setters
	let subsystem_specific_setters =
		info.subsystems().iter().filter(|ssf| !ssf.wip).enumerate().map(|(idx, ssf)| {
			let fname = &ssf.name;
			let ftype = &ssf.generic;
			let subsystem_consumes = &ssf.consumes;
			// Remove placeholder at specific field position
			let impl_generics = &(placeholder_type[..idx]
				.iter()
				.chain(placeholder_type[idx + 1..].iter())
				.collect::<Vec<_>>());
			let fname_with = format_ident!("{}_with", fname);
			let fname_replace = format_ident!("replace_{}", fname);

			let uninit_generics = produce_setter_generic_permutation(
				ftype,
				idx,
				placeholder_type.as_slice(),
				&Ident::new("OverseerFieldUninit", Span::call_site()),
			);
			let return_type_generics = produce_setter_generic_permutation(
				ftype,
				idx,
				placeholder_type.as_slice(),
				&Ident::new("OverseerFieldInit", Span::call_site()),
			);
			let other_field_name =
				field_name[..idx].iter().chain(field_name[idx + 1..].iter()).collect::<Vec<_>>();
			let replace_modified_generics = return_type_generics
				.iter()
				.enumerate()
				.map(|(other_idx, generic)| {
					if other_idx == idx {
						quote!(OverseerFieldInit<NEW>)
					} else {
						generic.clone()
					}
				})
				.collect::<Vec<_>>();

			quote! {
				impl <S, #ftype, #( #impl_generics, )*>
				#builder <S, #( #uninit_generics, )*> #builder_where_clause {
					/// Specify the subsystem in the builder directly
					pub fn #fname (self, var: #ftype ) ->
						#builder <S, #( #return_type_generics, )*>
					{
						#builder {
							#fname: OverseerFieldInit::<#ftype>::Value(var),
							#(
								#other_field_name: self.#other_field_name,
							)*
							spawner: self.spawner,
						}
					}
					/// Specify the the init function for a subsystem
					pub fn #fname_with<'a, F>(self, subsystem_init_fn: F ) ->
						#builder <S, #( #return_type_generics, )*>
						where
						F: 'static + FnOnce(#handle) ->
							::std::result::Result<#ftype, #error_ty>,
					{
						let boxed_func = OverseerFieldInit::<#ftype>::Fn(
							Box::new(subsystem_init_fn) as SubsystemInitFn<#ftype>
						);
						#builder {
							#fname: boxed_func,
							#(
								#other_field_name: self.#other_field_name,
							)*
							spawner: self.spawner,
						}
					}
				}

				impl <S, #ftype, #( #impl_generics, )*>
				#builder <S, #( #return_type_generics, )*> #builder_where_clause {
					/// Replace a subsystem by another implementation for the
					/// consumable message type.
					pub fn #fname_replace<NEW, F>(self, gen_replacement_fn: F)
						-> #builder <S, #( #replace_modified_generics, )*>
					where
						#ftype: 'static,
						F: 'static + FnOnce(#ftype) -> NEW,
						NEW: #support_crate ::Subsystem<#subsystem_ctx_name< #subsystem_consumes >, #error_ty>,
					{
						let replacement: OverseerFieldInit<NEW> = match self.#fname {
							OverseerFieldInit::Fn(fx) =>
								OverseerFieldInit::Fn(Box::new(move |handle: #handle| {
								let orig = fx(handle)?;
								Ok(gen_replacement_fn(orig))
							})),
							OverseerFieldInit::Value(val) =>
								OverseerFieldInit::Value(gen_replacement_fn(val)),
						};
						#builder {
							#fname: replacement,
							#(
								#other_field_name: self.#other_field_name,
							)*
							spawner: self.spawner,
						}
					}
				}
			}
		});
	// Produce setters for all baggage fields as well
	let baggage_specific_setters = info.baggage().iter().enumerate().map(|(idx, bag_field)| {
		// Baggage fields follow subsystems
		let baggage_idx = idx + subsystem_name.len();
		let fname = &bag_field.field_name;
		let ftype = &bag_field.field_ty;
		let impl_generics = placeholder_type[..baggage_idx]
			.iter()
			.chain(placeholder_type[baggage_idx + 1..].iter());
		let other_field_name =
			field_name[..baggage_idx].iter().chain(field_name[baggage_idx + 1..].iter());

		let uninit_generics = produce_setter_generic_permutation(
			ftype,
			baggage_idx,
			placeholder_type.as_slice(),
			&Ident::new("OverseerFieldUninit", Span::call_site()),
		);
		let return_type_generics = produce_setter_generic_permutation(
			ftype,
			baggage_idx,
			placeholder_type.as_slice(),
			&Ident::new("OverseerFieldInit", Span::call_site()),
		);
		let additional_baggage_generic = if bag_field.generic {
			quote! {#ftype,}
		} else {
			quote!()
		};

		quote! {
			impl <S, #additional_baggage_generic #( #impl_generics, )*>
			#builder <S, #( #uninit_generics, )*> #builder_where_clause {
				/// Specify the baggage in the builder
				pub fn #fname (self, var: #ftype ) ->
					#builder <S, #( #return_type_generics, )*>
				{
					#builder {
						#fname: OverseerFieldInit::<#ftype>::Value(var),
						#(
							#other_field_name: self.#other_field_name,
						)*
						spawner: self.spawner,
					}
				}
			}
		}
	});

	let event = &info.extern_event_ty;

	let mut ts = quote! {
		/// Convenience alias.
		type SubsystemInitFn<T> = Box<dyn FnOnce(#handle) -> ::std::result::Result<T, #error_ty> >;
		/// Type for the initialised field of the overseer builder
		pub enum OverseerFieldInit<T> {
			/// Defer initialization to a point where the `handle` is available.
			Fn(SubsystemInitFn<T>),
			/// Directly initialize the subsystem with the given subsystem type `T`.
			/// Also used for baggage fields
			Value(T),
		}
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
			pub fn builder() ->
				#builder<S, #( OverseerFieldUninit::<#field_type>, )* >
				#builder_where_clause
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

		#[allow(missing_docs)]
		pub struct #builder <S, #( #placeholder_type, )*> {
			#(
				#field_name: #placeholder_type,
			)*
			spawner: ::std::option::Option< S >,
		}

		impl<#builder_generics> #builder<S, #( OverseerFieldUninit::<#field_type>, )*>
			#builder_where_clause
		{
			fn new() -> Self {
				// explicitly assure the required traits are implemented
				fn trait_from_must_be_implemented<E>()
				where
					E: std::error::Error + Send + Sync + 'static + From<#support_crate ::OverseerError>
				{}

				trait_from_must_be_implemented::< #error_ty >();
				Self {
					#(
						#field_name: OverseerFieldUninit::<#field_type>::default(),
					)*
					spawner: None,
				}
			}
		}

		impl<S, #( #placeholder_type, )*> #builder<S, #( #placeholder_type, )*>
			#builder_where_clause
		{
			/// The spawner to use for spawning tasks.
			pub fn spawner(mut self, spawner: S) -> Self
			{
				self.spawner = Some(spawner);
				self
			}
		}

		impl<#builder_generics> #builder<S, #( OverseerFieldInit::<#field_type>, )*>
			#builder_where_clause {
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
					let #subsystem_name = match self. #subsystem_name {
						OverseerFieldInit::Fn(func) => func(handle.clone())?,
						OverseerFieldInit::Value(val) => val,
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
						#baggage_name: match self. #baggage_name {
							OverseerFieldInit::Value(val) => val,
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
	};

	ts.extend(baggage_specific_setters);
	ts.extend(subsystem_specific_setters);
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

/// This is a helper function that produces placeholder types for all
/// subsystems and baggage fields of the overseer
fn create_all_placeholder_idents(
	subsystems_names: &[Ident],
	baggage_names: &[Ident],
) -> Vec<Ident> {
	let mut result = Vec::<Ident>::with_capacity(subsystems_names.len() + baggage_names.len());

	result.extend(
		subsystems_names
			.iter()
			.map(|field_name| format_ident!("__{}", field_name.to_string().to_class_case())),
	);
	result.extend(
		baggage_names
			.iter()
			.map(|field_name| format_ident!("__{}", field_name.to_string().to_class_case())),
	);
	result
}

/// This helper function is used to create a replacement for a placeholder types
/// replacing a field matches specific index with a replacement ident
/// For example, we have `<A, B, C>` and we want to produce `<A, Init<T>, C>`,
/// so we call this function for `idx=1` and replacement ident as `Init<T>`
fn produce_setter_generic_permutation<T>(
	field_type: T,
	field_idx: usize,
	placeholder_types: &[Ident],
	replacement_type: &Ident,
) -> Vec<TokenStream>
where
	T: ToTokens,
{
	let replaced_type = quote! {#replacement_type<#field_type>};
	placeholder_types
		.iter()
		.enumerate()
		.map(|(other_idx, placeholder_ty)| {
			if other_idx == field_idx {
				replaced_type.clone()
			} else {
				quote! {#placeholder_ty}
			}
		})
		.collect::<Vec<_>>()
}

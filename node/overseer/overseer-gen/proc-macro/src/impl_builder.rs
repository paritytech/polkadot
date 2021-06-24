use quote::quote;
use syn::{Ident, Result};

use super::*;

/// Implement a builder pattern for the `Overseer`-type,
/// which acts as the gateway to constructing the overseer.
pub(crate) fn impl_builder(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let overseer_name = info.overseer_name.clone();
	let builder = Ident::new(&(overseer_name.to_string() + "Builder"), overseer_name.span());
	let handler = Ident::new(&(overseer_name.to_string() + "Handler"), overseer_name.span());

	let subsystem_name = &info.subsystem_names();
	let builder_generic_ty = &info.builder_generic_types();

	let channel_name = &info.channel_names("");
	let channel_name_unbounded = &info.channel_names("_unbounded");

	let channel_name_tx = &info.channel_names("_tx");
	let channel_name_unbounded_tx = &info.channel_names("_unbounded_tx");

	let channel_name_rx = &info.channel_names("_rx");
	let channel_name_unbounded_rx = &info.channel_names("_unbounded_rx");

	let baggage_generic_ty = &info.baggage_generic_types();
	let baggage_name = &info.baggage_names();
	let baggage_ty = &info.baggage_types();

	let error_ty = &info.extern_error_ty;

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

	let generics = quote! {
		< S, #( #baggage_generic_ty, )* >
	};
	let where_clause = quote! {
		where
			S: ::polkadot_overseer_gen::SpawnNamed,
	};

	let builder_generics = quote! {
		<S, #( #baggage_generic_ty, )* #( #builder_generic_ty, )* >
	};

	// all subsystems must have the same context
	// even if the overseer does not impose such a limit.
	let builder_additional_generics = quote! {
		<#( #builder_generic_ty, )* >
	};

	let consumes = &info.consumes();

	let subsyste_ctx_name = Ident::new(&(overseer_name.to_string() + "SubsystemContext"), overseer_name.span());

	let builder_where_clause = quote! {
		where
			S: ::polkadot_overseer_gen::SpawnNamed,
		#(
			#builder_generic_ty : Subsystem<#subsyste_ctx_name< #consumes >, #error_ty>,
		)*
	};

	let event = &info.extern_event_ty;

	let mut ts = quote! {
		impl #generics #overseer_name #generics #where_clause {
			/// Create a new overseer utilizing the builder.
			pub fn builder #builder_additional_generics () -> #builder #builder_generics
				#builder_where_clause
			{
				#builder :: default()
			}
		}

		/// Handler for an overseer.
		pub type #handler = ::polkadot_overseer_gen::metered::MeteredSender< #event >;

		#[allow(missing_docs)]
		pub struct #builder #builder_generics {
			#(
				#subsystem_name : ::std::option::Option< #builder_generic_ty >,
			)*
			#(
				#baggage_name : ::std::option::Option< #baggage_ty >,
			)*
			spawner: ::std::option::Option< S >,
		}

		impl #builder_generics Default for #builder #builder_generics {
			fn default() -> Self {
				// explicitly assure the required traits are implemented
				fn trait_from_must_be_implemented<E>()
				where E: std::error::Error + Send + Sync + 'static + From<::polkadot_overseer_gen::OverseerError>
				{}

				trait_from_must_be_implemented::< #error_ty >();

				Self {
				#(
					#subsystem_name: None,
				)*
				#(
					#baggage_name: None,
				)*
					spawner: None,
				}
			}
		}

		impl #builder_generics #builder #builder_generics #builder_where_clause {
			/// The spawner to use for spawning tasks.
			pub fn spawner(mut self, spawner: S) -> Self where S: ::polkadot_overseer_gen::SpawnNamed + Send {
				self.spawner = Some(spawner);
				self
			}

			#(
				/// Specify the particular subsystem implementation.
				pub fn #subsystem_name (mut self, subsystem: #builder_generic_ty ) -> Self {
					self. #subsystem_name = Some( subsystem );
					self
				}
			)*

			#(
				/// Attach the user defined addendum type.
				pub fn #baggage_name (mut self, baggage: #baggage_ty ) -> Self {
					self. #baggage_name = Some( baggage );
					self
				}
			)*

			/// Complete the construction and create the overseer type.
			pub fn build(mut self) -> ::std::result::Result<(#overseer_name #generics, #handler), #error_ty>
			{
				let (events_tx, events_rx) = ::polkadot_overseer_gen::metered::channel::<
					#event
				>(SIGNAL_CHANNEL_CAPACITY);

				let handler: #handler = events_tx.clone();

				let (to_overseer_tx, to_overseer_rx) = ::polkadot_overseer_gen::metered::unbounded::<
					ToOverseer
				>();

				#(
					let (#channel_name_tx, #channel_name_rx)
					=
						::polkadot_overseer_gen::metered::channel::<
							MessagePacket< #consumes >
						>(CHANNEL_CAPACITY);
				)*

				#(
					let (#channel_name_unbounded_tx, #channel_name_unbounded_rx) =
						::polkadot_overseer_gen::metered::unbounded::<
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

				let mut running_subsystems = ::polkadot_overseer_gen::FuturesUnordered::<
						BoxFuture<'static, ::std::result::Result<(), #error_ty > >
					>::new();

				#(
					// TODO generate a builder pattern that ensures this
					let #subsystem_name = self. #subsystem_name .expect("All subsystem must exist with the builder pattern.");

					let unbounded_meter = #channel_name_unbounded_rx.meter().clone();

					let message_rx: SubsystemIncomingMessages< #consumes > = ::polkadot_overseer_gen::select(
						#channel_name_rx, #channel_name_unbounded_rx
					);
					let (signal_tx, signal_rx) = ::polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
					let ctx = #subsyste_ctx_name::< #consumes >::new(
						signal_rx,
						message_rx,
						channels_out.clone(),
						to_overseer_tx.clone(),
					);

					let #subsystem_name: OverseenSubsystem< #consumes > =
						spawn::<_,_, #blocking, _, _, _>(
							&mut spawner,
							#channel_name_tx,
							signal_tx,
							unbounded_meter,
							channels_out.clone(),
							ctx,
							#subsystem_name,
							&mut running_subsystems,
						)?;
				)*

				#(
					let #baggage_name = self. #baggage_name .expect("Baggage must initialized");
				)*

				use ::polkadot_overseer_gen::StreamExt;

				let to_overseer_rx = to_overseer_rx.fuse();
				let overseer = #overseer_name {
					#(
						#subsystem_name,
					)*

					#(
						#baggage_name,
					)*

					spawner,
					running_subsystems,
					events_rx,
					to_overseer_rx,
				};

				Ok((overseer, handler))
			}
		}
	};
	ts.extend(impl_task_kind(info)?);
	Ok(ts)
}

pub(crate) fn impl_task_kind(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let signal = &info.extern_signal_ty;
	let error_ty = &info.extern_error_ty;

	let ts = quote! {

		use ::polkadot_overseer_gen::FutureExt as _;

		/// Task kind to launch.
		pub trait TaskKind {
			/// Spawn a task, it depends on the implementer if this is blocking or not.
			fn launch_task<S: SpawnNamed>(spawner: &mut S, name: &'static str, future: BoxFuture<'static, ()>);
		}

		#[allow(missing_docs)]
		struct Regular;
		impl TaskKind for Regular {
			fn launch_task<S: SpawnNamed>(spawner: &mut S, name: &'static str, future: BoxFuture<'static, ()>) {
				spawner.spawn(name, future)
			}
		}

		#[allow(missing_docs)]
		struct Blocking;
		impl TaskKind for Blocking {
			fn launch_task<S: SpawnNamed>(spawner: &mut S, name: &'static str, future: BoxFuture<'static, ()>) {
				spawner.spawn_blocking(name, future)
			}
		}

		/// Spawn task of kind `self` using spawner `S`.
		pub fn spawn<S, M, TK, Ctx, E, SubSys>(
			spawner: &mut S,
			message_tx: ::polkadot_overseer_gen::metered::MeteredSender<MessagePacket<M>>,
			signal_tx: ::polkadot_overseer_gen::metered::MeteredSender< #signal >,
			// meter for the unbounded channel
			unbounded_meter: ::polkadot_overseer_gen::metered::Meter,
			// connection to the subsystems
			channels_out: ChannelsOut,
			ctx: Ctx,
			s: SubSys,
			futures: &mut ::polkadot_overseer_gen::FuturesUnordered<BoxFuture<'static, ::std::result::Result<(), #error_ty> >>,
		) -> ::std::result::Result<OverseenSubsystem<M>, #error_ty>
		where
			S: ::polkadot_overseer_gen::SpawnNamed,
			M: std::fmt::Debug + Send + 'static,
			TK: TaskKind,
			Ctx: ::polkadot_overseer_gen::SubsystemContext<Message=M>,
			E: std::error::Error + Send + Sync + 'static + From<::polkadot_overseer_gen::OverseerError>,
			SubSys: ::polkadot_overseer_gen::Subsystem<Ctx, E>,
		{
			let ::polkadot_overseer_gen::SpawnedSubsystem { future, name } = s.start(ctx);

			let (tx, rx) = ::polkadot_overseer_gen::oneshot::channel();

			let fut = Box::pin(async move {
				if let Err(e) = future.await {
					::polkadot_overseer_gen::tracing::error!(subsystem=name, err = ?e, "subsystem exited with error");
				} else {
					::polkadot_overseer_gen::tracing::debug!(subsystem=name, "subsystem exited without an error");
				}
				let _ = tx.send(());
			});

			<TK as TaskKind>::launch_task(spawner, name, fut);

			futures.push(Box::pin(
				rx.map(|e| {
					tracing::warn!(err = ?e, "dropping error");
					Ok(())
				})
			));

			let instance = Some(SubsystemInstance {
				meters: ::polkadot_overseer_gen::SubsystemMeters {
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

	Ok(ts)
}

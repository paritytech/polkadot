use quote::quote;
use syn::{Ident, Result};

use super::*;

pub(crate) fn impl_overseer_struct(
	info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {
	let message_wrapper = &info.message_wrapper.clone();
	let overseer_name = info.overseer_name.clone();
	let subsystem_name = &info.subsystem_names();

	let builder_generic_ty = &info.builder_generic_types();

	let baggage_name = &info.baggage_names();
	let baggage_ty = &info.baggage_types();

	let baggage_generic_ty = &dbg!(info.baggage_generic_types());

	let generics = quote! {
		< S, #( #baggage_generic_ty, )* >
	};

	let where_clause = quote! {
		where
			S: ::polkadot_overseer_gen::SpawnNamed,
	};
	// FIXME add where clauses for baggage types

	let consumes = &info.consumes();

	let signal_ty = &info.extern_signal_ty;

	let event_ty = &info.extern_event_ty;

	let message_channel_capacity = info.message_channel_capacity;
	let signal_channel_capacity = info.signal_channel_capacity;

	let log_target = syn::LitStr::new(overseer_name.to_string().to_lowercase().as_str(), overseer_name.span());

	let mut ts = quote! {
		const STOP_DELAY: ::std::time::Duration = ::std::time::Duration::from_secs(1);

		/// Capacity of a bounded message channel between overseer and subsystem
		/// but also for bounded channels between two subsystems.
		const CHANNEL_CAPACITY: usize = #message_channel_capacity;

		/// Capacity of a signal channel between a subsystem and the overseer.
		const SIGNAL_CHANNEL_CAPACITY: usize = #signal_channel_capacity;

		/// The log target tag.
		const LOG_TARGET: &'static str = #log_target;

		/// The overseer.
		pub struct #overseer_name #generics {
			// Subsystem instances.
			#(
				#subsystem_name: OverseenSubsystem< #consumes >,
			)*

			// Non-subsystem members.
			#(
				#baggage_name: #baggage_ty,
			)*

			/// Responsible for driving the subsystem futures.
			spawner: S,

			/// The set of running subsystems.
			running_subsystems: ::polkadot_overseer_gen::FuturesUnordered<
				BoxFuture<'static, SubsystemResult<()>>
			>,

			/// Gather running subsystems' outbound streams into one.
			to_overseer_rx: ::polkadot_overseer_gen::Fuse<
				metered::UnboundedMeteredReceiver< ToOverseer >
			>,

			/// Events that are sent to the overseer from the outside world.
			events_rx: ::polkadot_overseer_gen::metered::MeteredReceiver< #event_ty >,
		}

		impl #generics #overseer_name #generics #where_clause {
			pub async fn broadcast_signal(&mut self, signal: #signal_ty) -> SubsystemResult<()> {
				#(
					self. #subsystem_name .send_signal(signal.clone()).await;
				)*
				let _ = signal;

				Ok(())
			}

			pub async fn route_message(&mut self, message: #message_wrapper) -> SubsystemResult<()> {
				match message {
					#(
						#message_wrapper :: #consumes ( inner ) => self. #subsystem_name .send_message( inner ).await?,
					)*
				}
				Ok(())
			}
		}
	};

	ts.extend(impl_builder(info)?);
	ts.extend(impl_overseen_subsystem(info)?);
	Ok(ts)
}

/// Implement a builder pattern for the `Overseer`-type,
/// which acts as the gateway to constructing the overseer.
pub(crate) fn impl_builder(
	info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {
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

	let blocking = &info.subsystems().iter().map(|x| x.blocking).collect::<Vec<_>>();

	let generics = quote! {
		< S, #( #baggage_generic_ty, )* >
	};
	let where_clause = quote! {
		where
			S: ::polkadot_overseer_gen::SpawnNamed,
	};

	let builder_generics = quote! {
		<Ctx, S, #( #baggage_generic_ty, )* #( #builder_generic_ty, )* >
	};

	// all subsystems must have the same context
	// even if the overseer does not impose such a limit.
	let builder_additional_generics = quote! {
		< Ctx, #( #builder_generic_ty, )* >
	};

	let error_ty = &info.extern_error_ty;

	let builder_where_clause = quote! {
		where
			Ctx: SubsystemContext,
			S: ::polkadot_overseer_gen::SpawnNamed,
			#( #builder_generic_ty : Subsystem<Ctx, #error_ty>, )*
	};

	let consumes = &info.consumes();
	let message_wrapper = &info.message_wrapper;
	let event = &info.extern_event_ty;
	let signal = &info.extern_signal_ty;

	let ts = quote! {

		impl #generics #overseer_name #generics #where_clause {
			fn builder #builder_additional_generics () -> #builder #builder_generics
				#builder_where_clause
			{
				#builder :: default()
			}
		}


		pub type #handler = ::polkadot_overseer_gen::metered::MeteredSender< #event >;

		#[derive(Debug, Clone)]
		struct #builder #builder_generics {
			#(
				#subsystem_name : ::std::option::Option< #builder_generic_ty >,
			)*
			#(
				#baggage_name : ::std::option::Option< #baggage_ty >,
			)*
			spawner: ::std::option::Option< S >,
			_phantom_ctx: ::std::marker::PhantomData< Ctx >,
		}

		impl #builder_generics Default for #builder #builder_generics {
			fn default() -> Self {
				Self {
				#(
					#subsystem_name: None,
				)*
				#(
					#baggage_name: None,
				)*
					spawner: None,
					_phantom_ctx: ::std::marker::PhantomData,
				}
			}
		}

		impl #builder_generics #builder #builder_generics #builder_where_clause {
			#(
				pub fn #subsystem_name (mut self, subsystem: #builder_generic_ty ) -> Self {
					self. #subsystem_name = Some( subsystem );
					self
				}
			)*

			pub fn build<F>(mut self, create_subsystem_ctx: F) -> (#overseer_name #generics, #handler)
				where
					F: FnMut(
						::polkadot_overseer_gen::metered::MeteredReceiver< #signal >,
						SubsystemIncomingMessages< #message_wrapper >,
						ChannelsOut,
						::polkadot_overseer_gen::metered::UnboundedMeteredSender<ToOverseer>,
					) -> Ctx,
			{

				let (events_tx, events_rx) = ::polkadot_overseer_gen::metered::channel::<
					#event
				>(SIGNAL_CHANNEL_CAPACITY);

				let handler: #handler = events_tx.clone();

				let (to_overseer_tx, to_overseer_rx) = ::polkadot_overseer_gen::metered::unbounded::<
					ToOverseer
				>();

				let channels_out = {
					#(
						let (#channel_name_tx, #channel_name_rx) =
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

					ChannelsOut {
						#(
							#channel_name: #channel_name_tx,
						)*
						#(
							#channel_name_unbounded: #channel_name_unbounded_tx,
						)*
					}
				};

				let spawner = self.spawner.expect("Spawner is set. qed");

				let mut running_subsystems = ::polkadot_overseer_gen::FuturesUnordered::<
						BoxFuture<'static, SubsystemResult<()>>
					>::new();

				#(
					// FIXME generate a builder pattern that ensures this
					let #subsystem_name = self. #subsystem_name .expect("All subsystem must exist with the builder pattern.");

					let #subsystem_name: OverseenSubsystem< #consumes > = {

						let unbounded_meter = channels_out. #channel_name .meter().clone();

						let message_tx: ::polkadot_overseer_gen::metered::MeteredSender< MessagePacket< #consumes > >
							= channels_out. #channel_name .clone();

						let message_rx: SubsystemIncomingMessages< #message_wrapper > =
							::polkadot_overseer_gen::select(
								channels_out. #channel_name .clone(),
								channels_out. #channel_name_unbounded .clone(),
							);

						let (signal_tx, signal_rx) = ::polkadot_overseer_gen::metered::channel::< #signal >(SIGNAL_CHANNEL_CAPACITY);

						let ctx = create_subsystem_ctx(
							signal_rx,
							message_rx,
							channels_out.clone(),
							to_overseer_tx.clone(),
						);

						let ::polkadot_overseer_gen::SpawnedSubsystem { future, name } = #subsystem_name .start(ctx);

						let (terminated_tx, terminated_rx) = ::polkadot_overseer_gen::oneshot::channel();

						let fut = Box::pin(async move {
							if let Err(e) = future.await {
							} else {
							}
							let _ = terminated_tx.send(());
						});

						if #blocking {
							spawner.spawn_blocking(name, fut);
						} else {
							spawner.spawn(name, fut);
						}

						running_subsystems.push(Box::pin(terminated_rx.map(|e| {
							Ok(())
						})));

						let instance = Some(
							SubsystemInstance::< #consumes > {
								meters: SubsystemMeters {
									unbounded: unbounded_meter,
									bounded: message_tx.meter().clone(),
									signals: signal_tx.meter().clone(),
								},
								tx_signal: signal_tx,
								tx_bounded: message_tx,
								signals_received: 0,
								name,
							}
						);

						OverseenSubsystem::< #consumes > {
							instance,
						}
					};
				)*

				#(
					let #baggage_name = self. #baggage_name .expect("Baggage must initialized");
				)*

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

				(overseer, handler)
			}
		}
	};
	Ok(ts)
}




pub(crate) fn impl_overseen_subsystem(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let signal = &info.extern_signal_ty;
	let message_wrapper = &info.message_wrapper;
	let consumes = &info.consumes();

	let ts = quote::quote! {

		/// A subsystem that the overseer oversees.
		///
		/// Ties together the [`Subsystem`] itself and it's running instance
		/// (which may be missing if the [`Subsystem`] is not running at the moment
		/// for whatever reason).
		///
		/// [`Subsystem`]: trait.Subsystem.html
		pub struct OverseenSubsystem<M> {
			pub instance: std::option::Option<
				SubsystemInstance<M, #signal>
			>,
		}

		impl<M> OverseenSubsystem<M> {
			/// Send a message to the wrapped subsystem.
			///
			/// If the inner `instance` is `None`, nothing is happening.
			pub async fn send_message(&mut self, message: M) -> SubsystemResult<()> {
				const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);

				if let Some(ref mut instance) = self.instance {
					match instance.tx_bounded.send(MessagePacket {
						signals_received: instance.signals_received,
						message: message.into(),
					}).timeout(MESSAGE_TIMEOUT).await
					{
						None => {
							Err(SubsystemError::SubsystemStalled(instance.name))
						}
						Some(res) => res.map_err(Into::into),
					}
				} else {
					Ok(())
				}
			}

			/// Send a signal to the wrapped subsystem.
			///
			/// If the inner `instance` is `None`, nothing is happening.
			pub async fn send_signal(&mut self, signal: #signal) -> SubsystemResult<()> {
				const SIGNAL_TIMEOUT: Duration = Duration::from_secs(10);

				if let Some(ref mut instance) = self.instance {
					match instance.tx_signal.send(signal).timeout(SIGNAL_TIMEOUT).await {
						None => {
							Err(SubsystemError::SubsystemStalled(instance.name))
						}
						Some(res) => {
							let res = res.map_err(Into::into);
							if res.is_ok() {
								instance.signals_received += 1;
							}
							res
						}
					}
				} else {
					Ok(())
				}
			}
		}
	};
	Ok(ts)
}

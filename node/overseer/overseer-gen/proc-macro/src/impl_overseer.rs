use quote::quote;
use syn::{Ident, Result};

use super::*;

pub(crate) fn impl_overseer_struct(
	info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {
	let message_wrapper = &info.message_wrapper.clone();
	let overseer_name = info.overseer_name.clone();
	let subsystem_name = &info.subsystem_names();

	let subsystem_generic_ty = &info.subsystem_generic_types();

	let baggage_name = &info.baggage_names();
	let baggage_ty = &info.baggage_types();

	let baggage_generic_ty = &info.baggage_generic_types();

	let generics = quote! {
		< Ctx, S, #( #baggage_generic_ty, )* >
	};

	let where_clause = quote! {
		where
			Ctx: SubsystemContext,
			S: SpawnNamed,
			#( #subsystem_generic_ty : Subsystem<Ctx>, )*
	};

	let consumes = &info.consumes();

	let message_channel_capacity = info.message_channel_capacity;
	let signal_channel_capacity = info.signal_channel_capacity;

	let mut ts = quote! {
		const CHANNEL_CAPACITY: usize = #message_channel_capacity;
		const SIGNAL_CHANNEL_CAPACITY: usize = #signal_channel_capacity;

		pub struct #overseer_name #generics {
			// Subsystem instances.
			#(
				#subsystem_name: OverseenSubsystem< SubsystemInstance < #consumes > >,
			)*

			// Non-subsystem members.
			#(
				#baggage_name: #baggage_ty,
			)*

			/// Responsible for driving the subsystem futures.
			spawner: S,

			/// The set of running subsystems.
			running_subsystems: FuturesUnordered<BoxFuture<'static, SubsystemResult<()>>>,

			/// Gather running subsystems' outbound streams into one.
			to_overseer_rx: Fuse<metered::UnboundedMeteredReceiver< ToOverseer >>,

			/// Events that are sent to the overseer from the outside world.
			events_rx: metered::MeteredReceiver<Event>,
		}

		impl #generics #overseer_name #generics #where_clause {
			pub async fn stop(mut self) {
				#(
					let _ = self. #subsystem_name .send_signal(OverseerSignal::Conclude).await;
				)*
				loop {
					select! {
						_ = self.running_subsystems.next() => {
							if self.running_subsystems.is_empty() {
								break;
							}
						},
						_ = stop_delay => break,
						complete => break,
					}
				}
			}
		}

		pub async fn broadcast_signal(&mut self, signal: OverseerSignal) -> SubsystemResult<()> {
			#(
				self. #subsystem_name .send_signal(signal.clone()).await;
			)*
			let _ = signal;

			Ok(())
		}

		pub async fn route_message(&mut self, msg: #message_wrapper) -> SubsystemResult<()> {
			match msg {
				#(
					#message_wrapper :: #consumes (msg) => self. #subsystem_name .send_message(msg).await?,
				)*
			}
			Ok(())
		}
	};

	ts.extend(impl_builder(info)?);
	Ok(ts)
}

/// Implement a builder pattern.
pub(crate) fn impl_builder(
	info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {
	let overseer_name = info.overseer_name.clone();
	let builder = Ident::new(&(overseer_name.to_string() + "Builder"), overseer_name.span());
	let handler = Ident::new(&(overseer_name.to_string() + "Handler"), overseer_name.span());

	let subsystem_name = &info.subsystem_names();
	let subsystem_generic_ty = &info.subsystem_generic_types();

	let channel_name = &info.channel_names("");
	let channel_name_unbounded = &info.channel_names("_unbounded");

	let channel_name_tx = &info.channel_names("_tx");
	let channel_name_unbounded_tx = &info.channel_names("_unbounded_tx");

	let channel_name_rx = &info.channel_names("_rx");
	let channel_name_unbounded_rx = &info.channel_names("_unbounded_rx");

	let baggage_generic_ty = &info.baggage_generic_types();
	let baggage_name = &info.baggage_names();

	let blocking = &info.subsystems().iter().map(|x| x.blocking).collect::<Vec<_>>();

	let generics = quote! {
		< Ctx, S, #( #baggage_generic_ty, )* >
	};

	let builder_generics = quote! {
		< Ctx, S, #( #baggage_generic_ty, )* #( #subsystem_generic_ty, )* >
	};

	let builder_additional_generics = quote! {
		< #( #subsystem_generic_ty, )* >
	};

	let where_clause = quote! {
		where
			Ctx: SubsystemContext,
			S: SpawnNamed,
			#( #subsystem_generic_ty : Subsystem<Ctx>, )*
	};

	let consumes = &info.consumes();
	let message_wrapper = &info.message_wrapper;

	let ts = quote! {

		impl #generics #overseer_name #generics #where_clause {
			fn builder::< #builder_additional_generics >() -> #builder #builder_generics {
				#builder :: default()
			}
		}

		#[derive(Debug, Clone, Default)]
		struct #builder #builder_generics {
			#(
				#subsystem_name : ::std::option::Option< #subsystem_generic_ty >,
			)*
			#(
				#baggage_name : ::std::option::Option< #baggage_name >,
			)*
			spawner: ::std::option::Option< S >,
		}

		impl #builder_generics #builder #builder_generics #where_clause {
			#(
				fn #subsystem_name (mut self, subsystem: #subsystem_generic_ty ) -> #builder {
					self.#subsystem_name = Some( new );
					self
				}
			)*

			fn build<F>(mut self, create_subsystem_ctx: F) -> (#overseer_name #generics, #handler)
				where F: FnMut(
					metered::MeteredReceiver<OverseerSignal>,
					meteted::SubsystemIncomingMessages< #message_wrapper >,
					ChannelsOut,
					metered::UnboundedMeteredSender<ToOverseer>,
				) -> Ctx,
			{

				let (events_tx, events_rx) = ::metered::channel(SIGNAL_CHANNEL_CAPACITY);

				let handler = #handler {
					events_tx: events_tx.clone(),
				};


				let (to_overseer_tx, to_overseer_rx) = metered::unbounded();

				let channels_out = {
					#(
						let (#channel_name_tx, #channel_name_rx) = ::metered::channel::<MessagePacket< #consumes >>(CHANNEL_CAPACITY);
					)*

					#(
						let (#channel_name_unbounded_tx, #channel_name_unbounded_rx) = ::metered::unbounded::<MessagePacket< #consumes >>();
					)*

					ChannelsOut {
						#(
							#channel_name: #channel_name_tx .clone(),
						)*
						#(
							#channel_name_unbounded: #channel_name_unbounded_tx .clone(),
						)*
					}
				};

				let spawner = &mut overseer.spawner;

				let mut running_subsystems = FuturesUnordered::<BoxFuture<'static, SubsystemResult<()>>>::new();

				#(
					let #subsystem_name: OverseenSubsystem<  #message_wrapper  > = {

						let unbounded_meter = #channel_name_unbounded_tx .meter().clone();

						let message_tx: metered::MeteredSender<MessagePacket< #message_wrapper >> = #channel_name_tx;

						let message_rx: SubsystemIncomingMessages< #message_wrapper > = stream::select(
							#channel_name_tx,
							#channel_name_unbounded_tx,
						);

						let (signal_tx, signal_rx) = metered::channel(SIGNAL_CHANNEL_CAPACITY);

						let ctx = create_subsystem_ctx(
							signal_rx,
							message_rx,
							channels_out.clone(),
							to_overseer_tx.clone(),
						);

						let SpawnedSubsystem { future, name } = #subsystem_name .start(ctx);

						let (terminated_tx, terminated_rx) = oneshot::channel();

						let fut = Box::pin(async move {
							if let Err(e) = future.await {
								tracing::error!(subsystem=name, err = ?e, "subsystem exited with error");
							} else {
								tracing::debug!(subsystem=name, "subsystem exited without an error");
							}
							let _ = terminated_tx.send(());
						});

						if #blocking {
							spawner.spawn_blocking(name, fut);
						} else {
							spawner.spawn(name, fut);
						}

						futures.push(Box::pin(terminated_rx.map(|e| { tracing::warn!(err = ?e, "dropping error"); Ok(()) })));

						let instance = Some(SubsystemInstance::< #message_wrapper > {
							meters: SubsystemMeters {
								unbounded: unbounded_meter,
								bounded: message_tx.meter().clone(),
								signals: signal_tx.meter().clone(),
							},
							tx_signal: signal_tx,
							tx_bounded: message_tx,
							signals_received: 0,
							name,
						});

						OverseenSubsystem::< #message_wrapper > {
							instance,
						}
					};
				)*

				let overseer = #overseer_name :: #generics {
					#(
						#subsystem_name,
					)*

					#(
						#baggage_name : self. #baggage_name .unwrap(),
					)*

					spawner,
					running_instance,
					events_rx,
					to_overseer_rx,
				};

				(overseer, handler)
			}
		}
	};
	Ok(ts)
}

use quote::quote;
use syn::{Ident, Result};

use super::*;

pub(crate) fn impl_overseer_struct(
	info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {
	let message_wrapper = &info.message_wrapper.clone();
	let overseer_name = dbg!(info.overseer_name.clone());
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
			S: ::polkadot_overseer_gen::SpawnNamed,
			#( #subsystem_generic_ty : Subsystem<Ctx>, )*
	};

	let consumes = &info.consumes();

	let signal_ty = &info.extern_event_ty;

	let message_channel_capacity = info.message_channel_capacity;
	let signal_channel_capacity = info.signal_channel_capacity;

	let log_target = syn::LitStr::new(overseer_name.to_string().to_lowercase().as_str(), overseer_name.span());

	let mut ts = quote! {
		const CHANNEL_CAPACITY: usize = #message_channel_capacity;
		const SIGNAL_CHANNEL_CAPACITY: usize = #signal_channel_capacity;
		const LOG_TARGET: &'static str = #log_target;
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
			running_subsystems: ::polkadot_overseer_gen::FuturesUnordered<BoxFuture<'static, SubsystemResult<()>>>,

			/// Gather running subsystems' outbound streams into one.
			to_overseer_rx: ::polkadot_overseer_gen::Fuse<metered::UnboundedMeteredReceiver< ToOverseer >>,

			/// Events that are sent to the overseer from the outside world.
			events_rx: ::polkadot_overseer_gen::metered::MeteredReceiver<Event>,
		}

		impl #generics #overseer_name #generics #where_clause {
			pub async fn stop(mut self) {
				unimplemented!("Stopping is not yet implemented")
			}
		}

		pub async fn broadcast_signal(&mut self, signal: #signal_ty) -> SubsystemResult<()> {
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
	ts.extend(impl_subsystem_instance(info)?);
	ts.extend(impl_overseen_subsystem(info)?);
	ts.extend(impl_trait_subsystem(info)?);
	ts.extend(impl_trait_subsystem_sender(info)?);

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
			S: ::polkadot_overseer_gen::SpawnNamed,
			#( #subsystem_generic_ty : Subsystem<Ctx>, )*
	};

	let consumes = &info.consumes();
	let message_wrapper = &info.message_wrapper;
	let event = &info.extern_event_ty;
	let signal = &info.extern_signal_ty;

	let ts = quote! {

		impl #generics #overseer_name #generics #where_clause {
			fn builder::< #builder_additional_generics >() -> #builder #builder_generics {
				#builder :: default()
			}
		}


		pub type #handler = ::polkadot_overseer_gen::metered::UnboundedMeteredSender< #event >;

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
				pub fn #subsystem_name (mut self, subsystem: #subsystem_generic_ty ) -> Self {
					self. #subsystem_name = Some( subsystem );
					self
				}
			)*

			pub fn build<F>(mut self, create_subsystem_ctx: F) -> (#overseer_name #generics, #handler)
				where F: FnMut(
					::polkadot_overseer_gen::metered::MeteredReceiver< #signal >,
					SubsystemIncomingMessages< #message_wrapper >,
					ChannelsOut,
					::polkadot_overseer_gen::metered::UnboundedMeteredSender<ToOverseer>,
				) -> Ctx,
			{

				let (events_tx, events_rx) = ::polkadot_overseer_gen:metered::channel(SIGNAL_CHANNEL_CAPACITY);

				let handler: #handler = events_tx.clone();

				let (to_overseer_tx, to_overseer_rx) = ::polkadot_overseer_gen::metered::unbounded();

				let channels_out = {
					#(
						let (#channel_name_tx, #channel_name_rx) = ::polkadot_overseer_gen::metered::channel::<MessagePacket< #consumes >>(CHANNEL_CAPACITY);
					)*

					#(
						let (#channel_name_unbounded_tx, #channel_name_unbounded_rx) = ::polkadot_overseer_gen::metered::unbounded::<MessagePacket< #consumes >>();
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

				let spawner = &mut overseer.spawner;

				let mut running_subsystems = ::polkadot_overseer_gen::FuturesUnordered::<BoxFuture<'static, SubsystemResult<()>>>::new();

				#(
					// FIXME generate a builder pattern that ensures this
					let #subsystem_name = self. #subsystem_name .expect("All subsystem must exist with the builder pattern.");

					let #subsystem_name: OverseenSubsystem<  #message_wrapper  > = {

						let unbounded_meter = channels_out. #channel_name .meter().clone();

						let message_tx: ::polkadot_overseer_gen::metered::MeteredSender<MessagePacket< #message_wrapper >> = channels_out. #channel_name .clone();

						let message_rx: SubsystemIncomingMessages< #message_wrapper > =
							::polkadot_overseer_gen::select(
								channels_out. #channel_name .clone(),
								channels_out. #channel_name_unbounded .clone(),
							);

						let (signal_tx, signal_rx) = ::polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);

						let ctx = create_subsystem_ctx(
							signal_rx,
							message_rx,
							channels_out.clone(),
							to_overseer_tx.clone(),
						);

						let ::polkadot_overseer_gen::SpawnedSubsystem { future, name } = #subsystem_name .start(ctx);

						let (terminated_tx, terminated_rx) = oneshot::channel();

						let fut = Box::pin(async move {
							if let Err(e) = future.await {
								::polkadot_overseer_gen::tracing::error!(subsystem=name, err = ?e, "subsystem exited with error");
							} else {
								::polkadot_overseer_gen::tracing::debug!(subsystem=name, "subsystem exited without an error");
							}
							let _ = terminated_tx.send(());
						});

						if #blocking {
							spawner.spawn_blocking(name, fut);
						} else {
							spawner.spawn(name, fut);
						}

						running_subsystems.push(Box::pin(terminated_rx.map(|e| { tracing::warn!(err = ?e, "dropping error"); Ok(()) })));

						let instance = Some(
							SubsystemInstance::< #message_wrapper > {
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





pub(crate) fn impl_subsystem_instance(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let signal = &info.extern_signal_ty;

	let ts = quote::quote! {
		/// A running instance of some [`Subsystem`].
		///
		/// [`Subsystem`]: trait.Subsystem.html
		///
		/// `M` here is the inner message type, and _not_ the generated `enum AllMessages`.
		pub struct SubsystemInstance<M> {
			tx_signal: ::polkadot_overseer_gen::metered::MeteredSender< #signal >,
			tx_bounded: ::polkadot_overseer_gen::metered::MeteredSender<MessagePacket<M>>,
			meters: SubsystemMeters,
			signals_received: usize,
			name: &'static str,
		}
	};

	Ok(ts)
}


pub(crate) fn impl_overseen_subsystem(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let signal = &info.extern_signal_ty;

	let ts = quote::quote! {


		/// A subsystem that we oversee.
		///
		/// Ties together the [`Subsystem`] itself and it's running instance
		/// (which may be missing if the [`Subsystem`] is not running at the moment
		/// for whatever reason).
		///
		/// [`Subsystem`]: trait.Subsystem.html
		pub struct OverseenSubsystem<M> {
			pub instance: std::option::Option<SubsystemInstance<M>>,
		}

		impl<M> OverseenSubsystem<M> {
			/// Send a message to the wrapped subsystem.
			///
			/// If the inner `instance` is `None`, nothing is happening.
			pub async fn send_message(&mut self, msg: M) -> SubsystemResult<()> {
				const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);

				if let Some(ref mut instance) = self.instance {
					match instance.tx_bounded.send(MessagePacket {
						signals_received: instance.signals_received,
						message: msg.into()
					}).timeout(MESSAGE_TIMEOUT).await
					{
						None => {
							::polkadot_overseer_gen::tracing::error!(target: crate::LOG_TARGET, "Subsystem {} appears unresponsive.", instance.name);
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
							::polkadot_overseer_gen::tracing::error!(target: crate::LOG_TARGET, "Subsystem {} appears unresponsive.", instance.name);
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


pub(crate) fn impl_trait_subsystem_sender(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let message_wrapper = &info.message_wrapper;

	let ts = quote! {
		#[::polkadot_overseer_gen::async_trait]
		pub trait SubsystemSender: Send + Clone + 'static {
			/// Send a direct message to some other `Subsystem`, routed based on message type.
			async fn send_message(&mut self, msg: #message_wrapper);

			/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
			async fn send_messages<T>(&mut self, msgs: T)
				where T: IntoIterator<Item = #message_wrapper> + Send, T::IntoIter: Send;

			/// Send a message onto the unbounded queue of some other `Subsystem`, routed based on message
			/// type.
			///
			/// This function should be used only when there is some other bounding factor on the messages
			/// sent with it. Otherwise, it risks a memory leak.
			fn send_unbounded_message(&mut self, msg: #message_wrapper);
		}
	};
	Ok(ts)
}



pub(crate) fn impl_trait_subsystem(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let message_wrapper = &info.message_wrapper;
	let error = &info.extern_error_ty;

	let ts = quote! {
		/// A message type that a subsystem receives from an overseer.
		/// It wraps signals from an overseer and messages that are circulating
		/// between subsystems.
		///
		/// It is generic over over the message type `M` that a particular `Subsystem` may use.
		#[derive(Debug)]
		pub enum FromOverseer<M,Signal> {
			/// Signal from the `Overseer`.
			Signal(Signal),

			/// Some other `Subsystem`'s message.
			Communication {
				/// Contained message
				msg: M,
			},
		}

		/// A context type that is given to the [`Subsystem`] upon spawning.
		/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
		/// or spawn jobs.
		///
		/// [`Overseer`]: struct.Overseer.html
		/// [`SubsystemJob`]: trait.SubsystemJob.html
		#[::polkadot_overseer_gen::async_trait]
		pub trait SubsystemContext: Send + 'static {
			/// The message type of this context. Subsystems launched with this context will expect
			/// to receive messages of this type.
			type Message: Send;
			type Signal: Send;

			/// Try to asynchronously receive a message.
			///
			/// This has to be used with caution, if you loop over this without
			/// using `pending!()` macro you will end up with a busy loop!
			async fn try_recv(&mut self) -> Result<Option<FromOverseer<Self::Message, Self::Signal>>, ()>;

			/// Receive a message.
			async fn recv(&mut self) -> SubsystemResult<FromOverseer<Self::Message, Self::Signal>>;

			/// Spawn a child task on the executor.
			async fn spawn(&mut self, name: &'static str, s: ::std::pin::Pin<Box<dyn ::polkadot_overseer_gen::Future<Output = ()> + Send>>) -> SubsystemResult<()>;

			/// Spawn a blocking child task on the executor's dedicated thread pool.
			async fn spawn_blocking(
				&mut self,
				name: &'static str,
				s: ::std::pin::Pin<Box<dyn ::polkadot_overseer_gen::Future<Output = ()> + Send>>,
			) -> SubsystemResult<()>;

			/// Send a direct message to some other `Subsystem`, routed based on message type.
			async fn send_message(&mut self, msg: #message_wrapper);

			/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
			async fn send_messages<T>(&mut self, msgs: T)
				where T: IntoIterator<Item = #message_wrapper> + Send, T::IntoIter: Send;
		}

		/// A trait that describes the [`Subsystem`]s that can run on the [`Overseer`].
		///
		/// It is generic over the message type circulating in the system.
		/// The idea that we want some type contaning persistent state that
		/// can spawn actually running subsystems when asked to.
		///
		/// [`Overseer`]: struct.Overseer.html
		/// [`Subsystem`]: trait.Subsystem.html
		pub trait Subsystem<Ctx: SubsystemContext> {
			/// Start this `Subsystem` and return `SpawnedSubsystem`.
			fn start(self, ctx: Ctx) -> SpawnedSubsystem < #error >;
		}
	};
	Ok(ts)
}

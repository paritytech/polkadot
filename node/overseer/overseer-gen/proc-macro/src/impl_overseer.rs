use quote::quote;
use syn::Result;

use super::*;

pub(crate) fn impl_overseer_struct(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let message_wrapper = &info.message_wrapper.clone();
	let overseer_name = info.overseer_name.clone();
	let subsystem_name = &info.subsystem_names();

	let baggage_name = &info.baggage_names();
	let baggage_ty = &info.baggage_types();

	let baggage_generic_ty = &info.baggage_generic_types();

	let generics = quote! {
		< S, #( #baggage_generic_ty, )* >
	};

	let where_clause = quote! {
		where
			S: ::polkadot_overseer_gen::SpawnNamed,
	};
	// FIXME add `where ..` clauses for baggage types

	let consumes = &info.consumes();

	let signal_ty = &info.extern_signal_ty;

	let error_ty = &info.extern_error_ty;

	let event_ty = &info.extern_event_ty;

	let message_channel_capacity = info.message_channel_capacity;
	let signal_channel_capacity = info.signal_channel_capacity;

	let log_target = syn::LitStr::new(overseer_name.to_string().to_lowercase().as_str(), overseer_name.span());

	let ts = quote! {
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
				BoxFuture<'static, ::std::result::Result<(), #error_ty>>
			>,

			/// Gather running subsystems' outbound streams into one.
			to_overseer_rx: ::polkadot_overseer_gen::stream::Fuse<
				::polkadot_overseer_gen::metered::UnboundedMeteredReceiver< ToOverseer >
			>,

			/// Events that are sent to the overseer from the outside world.
			events_rx: ::polkadot_overseer_gen::metered::MeteredReceiver< #event_ty >,
		}

		impl #generics #overseer_name #generics #where_clause {
			/// Send the given signal, a terminatin signal, to all subsystems
			/// and wait for all subsystems to go down.
			///
			/// The definition of a termination signal is up to the user and
			/// implementation specific.
			pub async fn wait_terminate(&mut self, signal: #signal_ty, timeout: ::std::time::Duration) -> ::polkadot_overseer_gen::SubsystemResult<()> {
				#(
					self. #subsystem_name .send_signal(signal.clone()).await;
				)*
				let _ = signal;

				let mut timeout_fut = ::polkadot_overseer_gen::Delay::new(
						timeout
					).fuse();

				loop {
					select! {
						_ = self.running_subsystems.next() => {
							if self.running_subsystems.is_empty() {
								break;
							}
						},
						_ = timeout_fut => break,
						complete => break,
					}
				}

				Ok(())
			}

			pub async fn broadcast_signal(&mut self, signal: #signal_ty) -> ::polkadot_overseer_gen::SubsystemResult<()> {
				#(
					self. #subsystem_name .send_signal(signal.clone()).await;
				)*
				let _ = signal;

				Ok(())
			}

			pub async fn route_message(&mut self, message: #message_wrapper) -> ::polkadot_overseer_gen::SubsystemResult<()> {
				match message {
					#(
						#message_wrapper :: #consumes ( inner ) => self. #subsystem_name .send_message( inner ).await?,
					)*
				}
				Ok(())
			}

			/// Extract information
			pub fn for_each_subsystem<Mapper, Output>(&self, mapper: Mapper)
			-> Vec<Output>
				where
				#(
					Mapper: for<'a> MapSubsystem<&'a OverseenSubsystem< #consumes >, Output=Output>,
				)*
			{
				vec![
				#(
					mapper.map_subsystem( & self. #subsystem_name ),
				)*
				]
			}
		}
	};

	Ok(ts)
}

pub(crate) fn impl_overseen_subsystem(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let signal = &info.extern_signal_ty;

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
				::polkadot_overseer_gen::SubsystemInstance<M, #signal>
			>,
		}

		impl<M> OverseenSubsystem<M> {
			/// Send a message to the wrapped subsystem.
			///
			/// If the inner `instance` is `None`, nothing is happening.
			pub async fn send_message(&mut self, message: M) -> ::polkadot_overseer_gen::SubsystemResult<()> {
				const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);

				if let Some(ref mut instance) = self.instance {
					match instance.tx_bounded.send(MessagePacket {
						signals_received: instance.signals_received,
						message: message.into(),
					}).timeout(MESSAGE_TIMEOUT).await
					{
						None => {
							Err(::polkadot_overseer_gen::SubsystemError::SubsystemStalled(instance.name))
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
			pub async fn send_signal(&mut self, signal: #signal) -> ::polkadot_overseer_gen::SubsystemResult<()> {
				const SIGNAL_TIMEOUT: ::std::time::Duration = ::std::time::Duration::from_secs(10);

				if let Some(ref mut instance) = self.instance {
					match instance.tx_signal.send(signal).timeout(SIGNAL_TIMEOUT).await {
						None => {
							Err(::polkadot_overseer_gen::SubsystemError::SubsystemStalled(instance.name))
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

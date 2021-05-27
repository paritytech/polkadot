use quote::quote;
use syn::{Ident, Result};

use super::*;

/// Implement a builder pattern for the `Overseer`-type,
/// which acts as the gateway to constructing the overseer.
pub(crate) fn impl_misc(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let overseer_name = info.overseer_name.clone();
	let subsystem_sender_name = Ident::new(&(overseer_name.to_string() + "SubsystemSender"), overseer_name.span());
	let subsystem_ctx_name = Ident::new(&(overseer_name.to_string() + "SubsystemContext"), overseer_name.span());
	let consumes = &info.consumes();
	let signal = &info.extern_signal_ty;

	let ts = quote! {
		#[derive(Debug, Clone)]
		pub struct #subsystem_sender_name {
			channels: ChannelsOut,
			signals_received: SignalsReceived,
		}

		#(
		#[::polkadot_overseer_gen::async_trait]
		impl SubsystemSender< #consumes > for #subsystem_sender_name {
			async fn send_message(&mut self, msg: #consumes) {
				self.channels.send_and_log_error(self.signals_received.load(), msg.into()).await;
			}

			async fn send_messages<T>(&mut self, msgs: T)
				where T: IntoIterator<Item = #consumes> + Send, T::IntoIter: Send
			{
				// This can definitely be optimized if necessary.
				for msg in msgs {
					self.send_message(msg).await;
				}
			}

			fn send_unbounded_message(&mut self, msg: #consumes) {
				self.channels.send_unbounded_and_log_error(self.signals_received.load(), msg.into());
			}
		}
		)*

		/// A context type that is given to the [`Subsystem`] upon spawning.
		/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
		/// or to spawn it's [`SubsystemJob`]s.
		///
		/// [`Overseer`]: struct.Overseer.html
		/// [`Subsystem`]: trait.Subsystem.html
		/// [`SubsystemJob`]: trait.SubsystemJob.html
		#[derive(Debug)]
		pub struct #subsystem_ctx_name<M>{
			signals: ::polkadot_overseer_gen::metered::MeteredReceiver< #signal >,
			messages: SubsystemIncomingMessages<M>,
			to_subsystems: #subsystem_sender_name,
			to_overseer: ::polkadot_overseer_gen::metered::UnboundedMeteredSender<
				::polkadot_overseer_gen::ToOverseer
				>,
			signals_received: SignalsReceived,
			pending_incoming: Option<(usize, M)>,
		}

		impl<M> #subsystem_ctx_name<M> {
			/// Create a new context.
			fn new(
				signals: ::polkadot_overseer_gen::metered::MeteredReceiver< #signal >,
				messages: SubsystemIncomingMessages<M>,
				to_subsystems: ChannelsOut,
				to_overseer: ::polkadot_overseer_gen::metered::UnboundedMeteredSender<ToOverseer>,
			) -> Self {
				let signals_received = SignalsReceived::default();
				#subsystem_ctx_name {
					signals,
					messages,
					to_subsystems: #subsystem_sender_name {
						channels: to_subsystems,
						signals_received: signals_received.clone(),
					},
					to_overseer,
					signals_received,
					pending_incoming: None,
				}
			}
		}

		#[::polkadot_overseer_gen::async_trait]
		impl<M: Send + 'static> SubsystemContext for #subsystem_ctx_name<M>
		where
			#subsystem_sender_name: polkadot_overseer_gen::SubsystemSender<M>,
		{
			type Message = M;
			type Signal = #signal;
			type Sender = #subsystem_sender_name;

			async fn try_recv(&mut self) -> Result<Option<FromOverseer<M, #signal>>, ()> {
				match ::polkadot_overseer_gen::poll!(self.recv()) {
					::polkadot_overseer_gen::Poll::Ready(msg) => Ok(Some(msg.map_err(|_| ())?)),
					::polkadot_overseer_gen::Poll::Pending => Ok(None),
				}
			}

			async fn recv(&mut self) -> ::polkadot_overseer_gen::SubsystemResult<FromOverseer<M, #signal>> {
				loop {
					// If we have a message pending an overseer signal, we only poll for signals
					// in the meantime.
					if let Some((needs_signals_received, msg)) = self.pending_incoming.take() {
						if needs_signals_received <= self.signals_received.load() {
							return Ok(::polkadot_overseer_gen::FromOverseer::Communication { msg });
						} else {
							self.pending_incoming = Some((needs_signals_received, msg));

							// wait for next signal.
							let signal = self.signals.next().await
								.ok_or(::polkadot_overseer_gen::SubsystemError::Context(
									"Signal channel is terminated and empty."
									.to_owned()
								))?;

							self.signals_received.inc();
							return Ok(::polkadot_overseer_gen::FromOverseer::Signal(signal))
						}
					}

					let mut await_message = self.messages.next().fuse();
					let mut await_signal = self.signals.next().fuse();
					let signals_received = self.signals_received.load();
					let pending_incoming = &mut self.pending_incoming;

					// Otherwise, wait for the next signal or incoming message.
					let from_overseer = ::polkadot_overseer_gen::futures::select_biased! {
						signal = await_signal => {
							let signal = signal
								.ok_or(::polkadot_overseer_gen::SubsystemError::Context(
									"Signal channel is terminated and empty."
									.to_owned()
								))?;

							::polkadot_overseer_gen::FromOverseer::Signal(signal)
						}
						msg = await_message => {
							let packet = msg
								.ok_or(::polkadot_overseer_gen::SubsystemError::Context(
									"Message channel is terminated and empty."
									.to_owned()
								))?;

							if packet.signals_received > signals_received {
								// wait until we've received enough signals to return this message.
								*pending_incoming = Some((packet.signals_received, packet.message));
								continue;
							} else {
								// we know enough to return this message.
								::polkadot_overseer_gen::FromOverseer::Communication { msg: packet.message}
							}
						}
					};

					if let ::polkadot_overseer_gen::FromOverseer::Signal(_) = from_overseer {
						self.signals_received.inc();
					}

					return Ok(from_overseer);
				}
			}

			fn sender(&mut self) -> &mut Self::Sender {
				&mut self.to_subsystems
			}

			#[deprecated(note="Avoid the message roundtrip and use `<_ as SubsystemContext>::spawn(ctx, name, fut)")]
			async fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
				-> ::polkadot_overseer_gen::SubsystemResult<()>
			{
				self.to_overseer.send(::polkadot_overseer_gen::ToOverseer::SpawnJob {
					name,
					s,
				}).await.map_err(Into::into)
			}

			#[deprecated(note="Avoid the message roundtrip and use `<_ as SubsystemContext>::spawn_blocking(ctx, name, fut)")]
			async fn spawn_blocking(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
				-> ::polkadot_overseer_gen::SubsystemResult<()>
			{
				self.to_overseer.send(::polkadot_overseer_gen::ToOverseer::SpawnBlockingJob {
					name,
					s,
				}).await.map_err(Into::into)
			}
		}
	};

	Ok(ts)
}

use quote::quote;
use syn::Result;

use super::*;

/// Generates the wrapper type enum.
pub(crate) fn impl_message_wrapper_enum(
	info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {
	let consumes = info.consumes();

    let message_wrapper = &info.message_wrapper;

	let msg = "Generated message type wrapper";
	let ts = quote! {
		#[doc = #msg]
		#[derive(Debug, Clone)]
		enum #message_wrapper {
			#(
				#consumes ( #consumes ),
			)*
		}

		#(
		impl ::std::convert::From<#consumes> for #message_wrapper {
			fn from(src: #consumes) -> Self {
				#message_wrapper :: #consumes ( src )
			}
		}
		)*

		#[derive(Debug, Clone)]
		pub struct OverseerSubsystemSender {
			channels: ChannelsOut,
			signals_received: SignalsReceived,
		}

		#[::polkadot_overseer_gen::async_trait]
		impl SubsystemSender for OverseerSubsystemSender {
			async fn send_message(&mut self, msg: #message_wrapper) {
				self.channels.send_and_log_error(self.signals_received.load(), msg).await;
			}

			async fn send_messages<T>(&mut self, msgs: T)
				where T: IntoIterator<Item = #message_wrapper> + Send, T::IntoIter: Send
			{
				// This can definitely be optimized if necessary.
				for msg in msgs {
					self.send_message(msg).await;
				}
			}

			fn send_unbounded_message(&mut self, msg: #message_wrapper) {
				self.channels.send_unbounded_and_log_error(self.signals_received.load(), msg);
			}
		}

	};

	Ok(ts)
}


use quote::quote;
use syn::Result;

use super::*;

/// Implement the helper type `ChannelsOut` and `MessagePacket<T>`.
pub(crate) fn impl_channels_out_struct(
    info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {

    let message_wrapper = info.message_wrapper.clone();

    let channel_name = &info.channel_names("");
    let channel_name_unbounded = &info.channel_names("_unbounded");

	let consumes = &info.consumes();

    let ts = quote! {
		pub struct ChannelsOut {
			#(
				pub #channel_name: ::metered::MeteredSender<MessagePacket< #consumes >>,
			)*
            #(
				pub #channel_name_unbounded: ::metered::UnboundedMeteredSender<MessagePacket< #consumes >>,
			)*
		}

        impl ChannelsOut {
            pub async fn send_and_log_error(
                &mut self,
                signals_received: usize,
                message: #message_wrapper,
            ) {
                let res = match message {
                #(
                    #message_wrapper :: #consumes (msg) => {
                        self. #channel_name .send(
                            make_packet(signals_received, msg)
                        ).await
                    },
                )*
                };

                if res.is_err() {
                    tracing::debug!(
                        target: LOG_TARGET,
                        "Failed to send a message to another subsystem",
                    );
                }
            }

            pub fn send_unbounded_and_log_error(
                &self,
                signals_received: usize,
                message: AllMessages,
            ) {
                let res = match message {
                #(
                    #message_wrapper :: #consumes (msg) => {
                        self. #channel_name_unbounded .send(
                            make_packet(signals_received, msg)
                        ).await
                    },
                )*
                };

                if res.is_err() {
                    tracing::debug!(
                        target: LOG_TARGET,
                        "Failed to send a message to another subsystem",
                    );
                }
            }
        }

	};
	Ok(ts)
}


use quote::quote;
use syn::{Ident, Result};

use super::*;

/// Implement the helper type `ChannelsOut` and `MessagePacket<T>`.
pub(crate) fn impl_channels_out_struct(
    info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {

    let message_wrapper = info.message_wrapper.clone();

    let channel_name = &info.channels("");
    let channel_name_unbounded = &info.channels("_unbounded");

	let mut consumes = &info.subsystem.iter().map(|ssf| ssf.consumes.clone()).collect::<Vec<_>>();

    let ts = quote! {
		#[derive(Debug)]
		struct MessagePacket<T> {
			signals_received: usize,
			message: T,
		}

		fn make_packet<T>(signals_received: usize, message: T) -> MessagePacket<T> {
			MessagePacket {
				signals_received,
				message,
			}
		}

		pub struct ChannelsOut {
			#(
				pub #channel_name: ::metered::MeteredSender<MessagePacket< #field_ty >>,
			)*
            #(
				pub #channel_name_unbounded: ::metered::UnboundedMeteredSender<MessagePacket< #field_ty >>,
			)*
		}

        impl ChannelsOut {
            async pub fn send_and_log_error(
                &mut self,
                signals_received: usize,
                message: #message_wrapper,
            ) {
                let res = match message {
                #(
                    #message_wrapper :: #field_ty (msg) => {
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
                    #message_wrapper :: #field_ty (msg) => {
                        self. #channel_name_unbounded .send(
                            make_packet(signals_received, msg)
                        ).await
                    },
                )*
                };

                if res.is_err() {
                    ::tracing::debug!(
                        target: LOG_TARGET,
                        "Failed to send a message to another subsystem",
                    );
                }
            }
        }

	};
	Ok(ts)
}

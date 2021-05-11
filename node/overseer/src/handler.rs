/// A handler used to communicate with the [`Overseer`].
///
/// [`Overseer`]: struct.Overseer.html
#[derive(Clone)]
pub struct OverseerHandler<M> {
	events_tx: metered::MeteredSender<Event>,
}

impl OverseerHandler {
	/// Inform the `Overseer` that that some block was imported.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	pub async fn block_imported(&mut self, block: BlockInfo) {
		self.send_and_log_error(Event::BlockImported(block)).await
	}

	/// Send some message to one of the `Subsystem`s.
	#[tracing::instrument(level = "trace", skip(self, msg), fields(subsystem = LOG_TARGET))]
	pub async fn send_msg(&mut self, msg: impl Into<M>) {
		self.send_and_log_error(Event::MsgToSubsystem(msg.into())).await
	}

	/// Inform the `Overseer` that some block was finalized.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	pub async fn block_finalized(&mut self, block: BlockInfo) {
		self.send_and_log_error(Event::BlockFinalized(block)).await
	}

	/// Wait for a block with the given hash to be in the active-leaves set.
	///
	/// The response channel responds if the hash was activated and is closed if the hash was deactivated.
	/// Note that due the fact the overseer doesn't store the whole active-leaves set, only deltas,
	/// the response channel may never return if the hash was deactivated before this call.
	/// In this case, it's the caller's responsibility to ensure a timeout is set.
	#[tracing::instrument(level = "trace", skip(self, response_channel), fields(subsystem = LOG_TARGET))]
	pub async fn wait_for_activation(&mut self, hash: Hash, response_channel: oneshot::Sender<SubsystemResult<()>>) {
		self.send_and_log_error(Event::ExternalRequest(ExternalRequest::WaitForActivation {
			hash,
			response_channel
		})).await
	}

	/// Tell `Overseer` to shutdown.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	pub async fn stop(&mut self) {
		self.send_and_log_error(Event::Stop).await
	}

	async fn send_and_log_error(&mut self, event: Event) {
		if self.events_tx.send(event).await.is_err() {
			tracing::info!(target: LOG_TARGET, "Failed to send an event to Overseer");
		}
	}
}

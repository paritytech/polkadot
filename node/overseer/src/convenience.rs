use crate::AllMessages;
use crate::OverseerSignal;
use crate::gen::{self, SubsystemSender};
use crate::SubsystemError;
use crate::{self as overseer};

/// A `Result` type that wraps [`SubsystemError`].
///
/// [`SubsystemError`]: struct.SubsystemError.html
pub type SubsystemResult<T> = Result<T, SubsystemError>;

// Simplify usage without having to do large scale modifications of all
// subsystems at once.


/// Specialized message type originating from the overseer.
pub type FromOverseer<M> = gen::FromOverseer<M, OverseerSignal>;

/// Specialized subsystem instance type of subsystems consuming a particular message type.
pub type SubsystemInstance<Message> = gen::SubsystemInstance<Message, OverseerSignal>;

/// Spawned subsystem.
pub type SpawnedSubsystem = gen::SpawnedSubsystem<SubsystemError>;

/// Convenience trait to reduce the number of trait parameters required.
#[async_trait::async_trait]
pub trait SubsystemContext: gen::SubsystemContext<
	Signal=OverseerSignal,
	AllMessages=AllMessages,
	Error=SubsystemError,
>
{
	/// The message type the subsystem consumes.
	type Message: std::fmt::Debug + Send + 'static;
	/// Sender type to communicate with other subsystems.
	type Sender: gen::SubsystemSender<AllMessages> + Send + Clone + 'static;

	/// Try to asynchronously receive a message.
	///
	/// This has to be used with caution, if you loop over this without
	/// using `pending!()` macro you will end up with a busy loop!
	async fn try_recv(&mut self) -> Result<Option<FromOverseer<<Self as SubsystemContext>::Message>>, ()>;

	/// Receive a message.
	async fn recv(&mut self) -> SubsystemResult<FromOverseer<<Self as SubsystemContext>::Message>>;

	/// Spawn a child task on the executor.
	async fn spawn(
		&mut self,
		name: &'static str,
		s: ::std::pin::Pin<Box<dyn futures::future::Future<Output = ()> + Send>>
	) -> SubsystemResult<()>;

	/// Spawn a blocking child task on the executor's dedicated thread pool.
	async fn spawn_blocking(
		&mut self,
		name: &'static str,
		s: ::std::pin::Pin<Box<dyn futures::future::Future<Output = ()> + Send>>,
	) -> SubsystemResult<()>;

	/// Obtain the sender.
	fn sender(&mut self) -> &mut <Self as SubsystemContext>::Sender;
}

#[async_trait::async_trait]
impl<T> gen::SubsystemContext for T
where
	T: SubsystemContext,
{
	type Message = <Self as SubsystemContext>::Message;
	type Sender = <Self as SubsystemContext>::Sender;
	type Signal = OverseerSignal;
	type AllMessages = AllMessages;
	type Error = SubsystemError;

	async fn try_recv(&mut self) -> Result<Option<overseer::gen::FromOverseer<<Self as SubsystemContext>::Message, Self::Signal>>, ()> {
		<Self as SubsystemContext>::try_recv(self).await
	}

	async fn recv(&mut self) -> Result<overseer::gen::FromOverseer<<Self as SubsystemContext>::Message, Self::Signal>, Self::Error> {
		<Self as SubsystemContext>::recv(self).await
	}

	async fn spawn(
		&mut self,
		name: &'static str,
		future: ::std::pin::Pin<Box<dyn futures::future::Future<Output = ()> + Send>>
	) -> Result<(), Self::Error> {
		<Self as SubsystemContext>::spawn(self, name, future).await
	}

	async fn spawn_blocking(
		&mut self,
		name: &'static str,
		future: ::std::pin::Pin<Box<dyn futures::future::Future<Output = ()> + Send>>,
	) -> Result<(), Self::Error> {
		<Self as SubsystemContext>::spawn_blocking(self, name, future).await
	}


	/// Obtain the sender.
	fn sender(&mut self) -> &mut <Self as SubsystemContext>::Sender {
		<Self as SubsystemContext>::sender(self)
	}
}

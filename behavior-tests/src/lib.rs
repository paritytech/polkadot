
use polkadot_service as service;
use sc_service::{error::Error as ServiceError, Configuration, TaskManager, TaskExecutor};
use sp_core::crypto::set_default_ss58_version;
use test_runner::default_config;
use log::info;
use sc_chain_spec::ChainSpec;
use futures::future::FutureExt;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error(transparent)]
	SubstrateService(#[from] sc_service::Error),


	#[error(transparent)]
	PolkadotService(#[from] polkadot_service::Error),

	#[error(transparent)]
	Io(#[from]std::io::Error)
}

pub type Result<T> = std::result::Result<T, Error>;


macro_rules! behavior_testcase {
	($name:ident => (&name:literal : $x:ty)* ) => {
		let _ = env_logger::Builder::new().is_test(true).try_init();


		for
	};
}

/// ```rust
bahvior_test!(alpha_omega => {
	"Alice"
}
/// ```


/// Launch a modified node with a given all subsystems instance
pub fn modded_node<Sel>(subsystemselection: Sel, chain_spec: Box<dyn ChainSpec>) -> Result<()> {

	// set_default_ss58_version(&chain_spec);

	let grandpa_pause = None;

	let jaeger_agent = None;

	let telemetry_worker = None;

	let mut tokio_rt = tokio::runtime::Builder::new()
		.threaded_scheduler()
		.enable_all()
		.build()?;


	let handle = tokio_rt.handle().clone();
	let task_executor: TaskExecutor = (move |future, _task_type| {
		handle.spawn(future).map(|_| ())
	}).into();


	let config = default_config(task_executor, chain_spec);
	let initialize = move |config: Configuration| async move {
		let task_manager =
		service::build_full(
				config,
				service::IsCollator::No,
				grandpa_pause,
				jaeger_agent,
				telemetry_worker,
			)
			.map(|full| full.task_manager)?;
		Ok::<_, Error>(task_manager)
	};

	let mut task_manager = tokio_rt.block_on(initialize(config))?;
	// futures::pin_mut!(task_manager);
	let res = tokio_rt.block_on(task_manager.future().fuse());
	tokio_rt.block_on(task_manager.clean_shutdown());
	Ok(())
}

#[cfg(test)]
mod tests {
	#[test]
	fn it_works() {
		assert_eq!(2 + 2, 4);
	}
}

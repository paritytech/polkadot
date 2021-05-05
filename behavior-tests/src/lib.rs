
use polkadot_service as service;
use sc_service::{error::Error as ServiceError, Configuration, TaskManager};
use sp_core::crypto::set_default_ss58_version;
use test_runner::default_config;
use log::info;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("foo")]
	Foo
}

pub type Result = std::result::Result<T, Error>;

/// Launch a modified node with a given all subsystems instance
pub fn modded_node<S>(subsystems: S, chain_spec: Box<dyn ChainSpec>) -> Result<()> {

	set_default_ss58_version(chain_spec);

	let grandpa_pause = None;

	let jaeger_agent = None;

	let telemetry_worker = None;

	let mut runtime = tokio::runtime::Builder::new()
		.threaded_scheduler()
		.enable_all()
		.build();

	let config = default_config();
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

	let mut task_manager = tokio.block_on(initialize(self.config))?;
	let res = tokio.block_on(main(task_manager.future().fuse()));
	tokio.block_on(task_manager.clean_shutdown());
}

#[cfg(test)]
mod tests {
	#[test]
	fn it_works() {
		assert_eq!(2 + 2, 4);
	}
}

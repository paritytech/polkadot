#![allow(dead_code)]

use polkadot_overseer_gen::*;

#[derive(Default)]
struct AwesomeSubSys;

impl ::polkadot_overseer_gen::Subsystem<OverseerSubsystemContext<MsgStrukt>, OverseerError> for AwesomeSubSys {
	fn start(self, _ctx: OverseerSubsystemContext<MsgStrukt>) -> SpawnedSubsystem<OverseerError> {
		unimplemented!("starting yay!")
	}
}

#[derive(Clone, Debug)]
pub struct SigSigSig;

pub struct Event;

#[derive(Clone, Debug)]
pub struct MsgStrukt(u8);

#[orchestra(signal=SigSigSig, error=OverseerError, event=Event, gen=AllMessages)]
struct Overseer {
	#[subsystem(MsgStrukt)]
	sub0: AwesomeSubSys,
	i_like_pie: f64,
}

#[derive(Debug, Clone)]
pub struct DummySpawner;

impl SpawnNamed for DummySpawner {
	fn spawn_blocking(
		&self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		_future: futures::future::BoxFuture<'static, ()>,
	) {
		unimplemented!("spawn blocking {} {}", task_name, subsystem_name.unwrap_or("default"))
	}

	fn spawn(
		&self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		_future: futures::future::BoxFuture<'static, ()>,
	) {
		unimplemented!("spawn {} {}", task_name, subsystem_name.unwrap_or("default"))
	}
}

struct DummyCtx;

fn main() {
	let _ = Overseer::builder()
		.sub0(AwesomeSubSys::default())
		.i_like_pie(std::f64::consts::PI)
		//.spawner(DummySpawner) // Spawner is missing
		.build()
		.unwrap();
}

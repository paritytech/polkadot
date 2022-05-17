#![allow(dead_code)]

use orchestra::*;

#[derive(Default)]
struct AwesomeSubSys;

impl ::orchestra::Subsystem<OrchestraSubsystemContext<MsgStrukt>, OrchestraError> for AwesomeSubSys {
	fn start(self, _ctx: OrchestraSubsystemContext<MsgStrukt>) -> SpawnedSubsystem<OrchestraError> {
		unimplemented!("starting yay!")
	}
}

#[derive(Clone, Debug)]
pub struct SigSigSig;

pub struct Event;

#[derive(Clone, Debug)]
pub struct MsgStrukt(u8);

#[orchestra(signal=SigSigSig, error=OrchestraError, event=Event, gen=AllMessages)]
struct Orchestra {
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
	let _ = Orchestra::builder()
		.sub0(AwesomeSubSys::default())
		//.i_like_pie(std::f64::consts::PI) // The filed is not initialised
		.spawner(DummySpawner)
		.build()
		.unwrap();
}

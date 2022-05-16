#![allow(dead_code)]

use polkadot_overseer_gen::*;

#[derive(Default)]
struct AwesomeSubSysA;


impl ::polkadot_overseer_gen::Subsystem<OverseerSubsystemContext<MsgA>, OverseerError> for AwesomeSubSysA {
	fn start(self, _ctx: OverseerSubsystemContext<MsgA>) -> SpawnedSubsystem<OverseerError> {
		SpawnedSubsystem { name: "sub A", future: Box::pin(async move { Ok(()) }) }
	}
}
impl ::polkadot_overseer_gen::Subsystem<OverseerSubsystemContext<MsgB>, OverseerError> for AwesomeSubSysB {
	fn start(self, _ctx: OverseerSubsystemContext<MsgB>) -> SpawnedSubsystem<OverseerError> {
		SpawnedSubsystem { name: "sub B", future: Box::pin(async move { Ok(()) }) }
	}
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
		println!("spawn blocking {} {}", task_name, subsystem_name.unwrap_or("default"))
	}

	fn spawn(
		&self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		_future: futures::future::BoxFuture<'static, ()>,
	) {
		println!("spawn {} {}", task_name, subsystem_name.unwrap_or("default"))
	}
}

#[derive(Default)]
struct AwesomeSubSysB;

#[derive(Clone, Debug)]
pub struct SigSigSig;

pub struct Event;

#[derive(Clone, Debug)]
pub struct MsgA(u8);

#[derive(Clone, Debug)]
pub struct MsgB(u8);

#[orchestra(signal=SigSigSig, event=Event, gen=AllMessages, error=OverseerError)]
pub struct Overseer {
	#[subsystem(MsgA)]
	sub_a: AwesomeSubSysA,

	#[subsystem(wip, MsgB)]
	sub_b: AwesomeSubSysB,
}

pub struct DummyCtx;

fn main() {
	let _overseer_builder = Overseer::builder()
		.sub_a(AwesomeSubSysA::default())
		// b is tagged as `wip`
		// .sub_b(AwesomeSubSysB::default())
		.spawner(DummySpawner)
		.build();
}

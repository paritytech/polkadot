use std::collections::HashSet;
use std::time::Duration;

use futures::pending;
use futures_timer::Delay;
use kv_log_macro as log;

use overseer::{Overseer, Subsystem, SubsystemContext, SubsystemJob};

struct Subsystem1;

impl Subsystem1 {
	async fn run(mut ctx: SubsystemContext<usize>)  {
		loop {
			match ctx.try_recv().await {
				Ok(Some(msg)) => {
					log::info!("Subsystem1 received message {}", msg);
				}
				Ok(None) => (),
				Err(_) => {}
			}

			Delay::new(Duration::from_secs(1)).await;
			ctx.send_msg(10).await;
			pending!();
		}
	}

	fn new() -> Self {
		Self
	}
}

impl Subsystem<usize> for Subsystem1 {
	fn start(&mut self, ctx: SubsystemContext<usize>) -> SubsystemJob {
		SubsystemJob(Box::pin(async move {
			Self::run(ctx).await;
			Ok(())
		}))
	}
}

struct Subsystem2;


impl Subsystem2 {
	async fn run(mut ctx: SubsystemContext<usize>)  {
		let ss3 = Box::new(Subsystem3);

		let ss3_id = ctx.spawn(ss3).await;
		log::info!("Received subsystem id {:?}", ss3_id);
		loop {
			match ctx.try_recv().await {
				Ok(Some(msg)) => {
					log::info!("Subsystem2 received message {}", msg);
				}
				Ok(None) => (),
				Err(_) => {}
			}
			pending!();
		}
	}

	fn new() -> Self {
		Self
	}
}

impl Subsystem<usize> for Subsystem2 {
	fn start(&mut self, ctx: SubsystemContext<usize>) -> SubsystemJob {
		SubsystemJob(Box::pin(async move {
			Self::run(ctx).await;
			Ok(())
		}))
	}
}

struct Subsystem3;

impl Subsystem<usize> for Subsystem3 {
	fn start(&mut self, mut ctx: SubsystemContext<usize>) -> SubsystemJob {
		SubsystemJob(Box::pin(async move {
			// TODO: ctx actually has to be used otherwise the channels are dropped
			loop {
				// ignore all incoming msgs
				while let Ok(Some(_)) = ctx.try_recv().await {
				}
				log::info!("Subsystem3 tick");
				Delay::new(Duration::from_secs(1)).await;

				pending!();
			}
		}))
	}

	fn can_recv_msg(&self, _msg: &usize) -> bool { false }
}

fn main() {
	femme::with_level(femme::LevelFilter::Trace);

	futures::executor::block_on(async {
		let subsystems: Vec<Box<dyn Subsystem<usize>>> = vec![
			Box::new(Subsystem1::new()),
			Box::new(Subsystem2::new()),
		];

		let overseer = Overseer::new(subsystems);
		overseer.run().await;
	});
}

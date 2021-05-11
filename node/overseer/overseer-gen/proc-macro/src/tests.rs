use super::*;
use quote::quote;

#[test]
fn basic() {
    let attr = quote! {
        overloard(event=OverseerSignal,gen=AllMessages)
    };

    let item = quote! {
        pub struct Ooooh<X = Pffffffft> where X: Secrit {
            #[subsystem(no_dispatch, Foo)]
            sub0: FooSubsystem,

            #[subsystem(blocking, Bar)]
            yyy: BaersBuyBilliardBalls,

            #[subsystem(no_dispatch, blocking, Twain)]
            fff: Beeeeep,

            #[subsystem(Rope)]
            mc: MountainCave,

            metrics: Metrics,
        }
    };

    let output = impl_overseer_gen(attr, item).expect("Simple example always works. qed");
    println!("//generated:");
    println!("{}", output);
}

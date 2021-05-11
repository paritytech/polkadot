use super::*;
use quote::quote;
use assert_matches::assert_matches;
use syn::parse_quote;

#[test]
fn basic() {
    let attr = quote! {
        (event=OverseerSignal,gen=AllMessages)
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


#[test]
fn attr_parsing_works() {
    let attr: AttrArgs = parse_quote! {
        (gen=AllMessage, event=::some::why::ExternEvent, signal_capacity=111, message_capacity=222,)
    };
    assert_matches!(attr, AttrArgs {
        message_channel_capacity,
        signal_channel_capacity,
        ..
    } => {
        assert_eq!(message_channel_capacity, 222);
        assert_eq!(signal_channel_capacity, 111);
    });
}

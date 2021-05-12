use super::*;
use quote::quote;
use assert_matches::assert_matches;
use syn::parse_quote;



#[test]
fn print() {
    let attr = quote! {
        gen=AllMessage, event=::some::why::ExternEvent, signal=SigSigSig, signal_capacity=111, message_capacity=222,
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
fn struct_parse_full() {
    let item: OverseerGuts = parse_quote! {
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
    let _ = dbg!(item);
}

#[test]
fn struct_parse_basic() {
    let item: OverseerGuts = parse_quote! {
        pub struct Ooooh {
            #[subsystem(Foo)]
            sub0: FooSubsystem,
        }
    };
    let _ = dbg!(item);
}


#[test]
fn attr_full() {
    let attr: AttrArgs = parse_quote! {
        gen=AllMessage, event=::some::why::ExternEvent, signal=SigSigSig, signal_capacity=111, message_capacity=222,
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


#[test]
fn attr_partial() {
    let attr: AttrArgs = parse_quote! {
        gen=AllMessage, event=::some::why::ExternEvent, signal=::foo::SigSigSig,
    };
    assert_matches!(attr, AttrArgs {
        message_channel_capacity,
        signal_channel_capacity,
        ..
    } => {
    });
}

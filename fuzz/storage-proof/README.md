# Storage Proof Fuzzer

## How to run?

Install dependencies:
```
$ sudo apt install build-essential binutils-dev libunwind-dev
```
or on nix:
```
$ nix-shell -p honggfuzz
```

Install `cargo hfuzz` plugin:
```
$ cargo install honggfuzz
```

Run:
```
$ cargo hfuzz run storage-proof-fuzzer
```

Use `HFUZZ_RUN_ARGS` to customize execution:
```
# 1 second of timeout
# use 12 fuzzing thread
# be verbose
# stop after 1000000 fuzzing iteration
# exit upon crash
HFUZZ_RUN_ARGS="-t 1 -n 12 -v -N 1000000 --exit_upon_crash" cargo hfuzz run example
```

More details in the [official documentation](https://docs.rs/honggfuzz/0.5.52/honggfuzz/#about-honggfuzz).

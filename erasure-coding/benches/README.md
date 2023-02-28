### Run benches
```
$ cd erasure-coding # ensure you are in the right directory
$ cargo bench
```

### `scaling_with_validators`

This benchmark evaluates the performance of constructing the chunks and the erasure root from PoV and
reconstructing the PoV from chunks. You can see the results of running this bench on 5950x below.
Interestingly, with `10_000` chunks (validators) its slower than with `50_000` for both construction
and reconstruction.
```
construct/200           time:   [93.924 ms 94.525 ms 95.214 ms]
                        thrpt:  [52.513 MiB/s 52.896 MiB/s 53.234 MiB/s]
construct/500           time:   [111.25 ms 111.52 ms 111.80 ms]
                        thrpt:  [44.721 MiB/s 44.837 MiB/s 44.946 MiB/s]
construct/1000          time:   [117.37 ms 118.28 ms 119.21 ms]
                        thrpt:  [41.941 MiB/s 42.273 MiB/s 42.601 MiB/s]
construct/2000          time:   [125.05 ms 125.72 ms 126.38 ms]
                        thrpt:  [39.564 MiB/s 39.772 MiB/s 39.983 MiB/s]
construct/10000         time:   [270.46 ms 275.11 ms 279.81 ms]
                        thrpt:  [17.869 MiB/s 18.174 MiB/s 18.487 MiB/s]
construct/50000         time:   [205.86 ms 209.66 ms 213.64 ms]
                        thrpt:  [23.404 MiB/s 23.848 MiB/s 24.288 MiB/s]

reconstruct/200         time:   [180.73 ms 184.09 ms 187.73 ms]
                        thrpt:  [26.634 MiB/s 27.160 MiB/s 27.666 MiB/s]
reconstruct/500         time:   [195.59 ms 198.58 ms 201.76 ms]
                        thrpt:  [24.781 MiB/s 25.179 MiB/s 25.564 MiB/s]
reconstruct/1000        time:   [207.92 ms 211.57 ms 215.57 ms]
                        thrpt:  [23.195 MiB/s 23.633 MiB/s 24.048 MiB/s]
reconstruct/2000        time:   [218.59 ms 223.68 ms 229.18 ms]
                        thrpt:  [21.817 MiB/s 22.354 MiB/s 22.874 MiB/s]
reconstruct/10000       time:   [496.35 ms 505.17 ms 515.42 ms]
                        thrpt:  [9.7008 MiB/s 9.8977 MiB/s 10.074 MiB/s]
reconstruct/50000       time:   [276.56 ms 277.53 ms 278.58 ms]
                        thrpt:  [17.948 MiB/s 18.016 MiB/s 18.079 MiB/s]
```

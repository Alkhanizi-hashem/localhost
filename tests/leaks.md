# Leak Check

Run at least one of these before the audit and keep the output:

## Valgrind

```sh
valgrind --leak-check=full --show-leak-kinds=all target/debug/localhost config/default.conf
```

Then hit the server with:

```sh
tests/stress.sh
```

## Sanitizers

```sh
RUSTFLAGS="-Zsanitizer=address" cargo +nightly build
```

## Long-run RSS smoke

```sh
target/debug/localhost config/default.conf &
tests/stress.sh
ps -o pid,rss,command -p "$!"
```

For the audit, keep one saved output showing no leak growth across repeated runs.

## Fallback without Valgrind

If `valgrind` is not available on the audit machine, run:

```sh
tests/leak_check.sh
```

That fallback starts the server, performs two repeated request bursts, records RSS before and after each burst, and checks that there are no remaining active socket states on port `8080` once the burst ends.

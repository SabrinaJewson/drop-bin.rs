# drop-bin

In Rust, values' destructors are automatically run when they go out of scope. However,
destructors can be expensive and so you may wish to defer running them until later, when your
program has some free time or memory usage is getting high. A bin allows you to put any number
of differently-typed values in it, and you can clear them all out, running their destructors,
whenever you want.

## Example

```rust
let bin = drop_bin::Bin::new();

let some_data = "Hello World!".to_owned();
bin.add(some_data);
// `some_data`'s destructor is not run.

bin.clear();
// `some_data`'s destructor has been run.
```

License: MIT OR Apache-2.0

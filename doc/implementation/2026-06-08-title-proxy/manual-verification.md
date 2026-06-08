# Manual Verification Checklist

## Interactive bash pass-through

```sh
cargo run -- -- bash
```

- [ ] Shell opens and accepts input
- [ ] Output prints correctly
- [ ] `exit` exits to retry prompt
- [ ] Result: ___ Date: ___ Terminal: ___ Shell: ___ Platform: ___

## Interactive SSH pass-through

```sh
cargo run -- -- ssh <user@host>
```

- [ ] SSH session behaves like direct `ssh`
- [ ] Result: ___ Date: ___

## Terminal resize propagation

```sh
cargo run -- -- sh -c 'stty size; sleep 5; stty size'
```

Resize the outer terminal during the sleep.

- [ ] Second `stty size` reflects the new terminal dimensions
- [ ] Result: ___ Date: ___

## Terminal restoration after normal quit

```sh
stty -g
cargo run -- -- sh -c 'exit 0'
# press q
stty -g
```

- [ ] Two `stty -g` outputs match
- [ ] Result: ___ Date: ___

## Terminal restoration after Ctrl+C at prompt

```sh
stty -g
cargo run -- -- sh -c 'exit 0'
# press Ctrl+C at prompt
stty -g
```

- [ ] Two `stty -g` outputs match
- [ ] Result: ___ Date: ___

## Terminal restoration after SIGTERM to proxy

```sh
stty -g
cargo run -- -- sh -c 'while true; do sleep 1; done'
# send SIGTERM to proxy from another terminal
stty -g
```

- [ ] Terminal remains usable and cooked
- [ ] Result: ___ Date: ___

## Title re-emit

```sh
cargo run -- --reemit-title true -- sh -c 'printf "\033]2;phase5\007"; exit 0'
```

- [ ] Captured title is re-emitted after child exit before prompt
- [ ] Result: ___ Date: ___

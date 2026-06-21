# title-preserver

> Preserve a window title from reverting on detach.

A longrunning terminal proxy that watches for and filters title changes, preventing the title reverting undetaching.

# TODO

- preserver mode: a filter of title updates to reject/ignore
- change title to append `[d]` when detached, reset on re-attach to original (not [d]) title
- integrate with `zim-tgo` teleport.
- better define attached state machine, transitions
- dreaming: materialized arrow cache view of system state

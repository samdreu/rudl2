# Simulation and Async/Await Diagrams

These diagrams document the runtime model of `copper-sim`, `Clock`, `tick().await`, `emit!`, and the delta-cycle scheduler.

Files:
- [component.mmd](component.mmd)
- [sequence.mmd](sequence.mmd)
- [state.mmd](state.mmd)
- [timing.mmd](timing.mmd)

Suggested reading order:
1. `sequence.mmd` for the runtime flow
2. `state.mmd` for task lifecycle and suspension
3. `component.mmd` for system boundaries
4. `timing.mmd` for cycle/delta ordering

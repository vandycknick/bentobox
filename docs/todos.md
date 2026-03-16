# TODOs

- [ ] Replace hdiutil for cidata generate with something rust native
- [ ] Machine.Start synchronously waits until a machine is runnig atleast thats how it is implemented for now in the VZ backend. But thats not how Stop works, it's just a signal at the moment and doesn't wait for the underlying machine state. This means in the stop command there's some ugly polling logic. Maybe it's nicer if neither start and stop wait and are just triggers, and I expose a WatchState function from the Machine api.
- [ ] Codesign doesn't work when running cargo build --release
- [ ] Error propagation doesn't work correctly. For example a non signed binary can't start VZ, but that error isn't correctly getting propaged to the cli. CLI just keeps waiting even though instanced has shutdown already. Also I would expect an erro log in the trace log and that doesn't exist either.
- [ ] Wrong guest-agent config crashes intsanced, but the start command will still wait until it timesout with no clear error message.
- [ ] Revisit extension architecture
- [ ] Implement metrics / logging endpoint

## Packages needed to build the kernel in arch

- base-devel
- bc

# TODOs

- [ ] Replace hdiutil for cidata generate with something rust native
- [ ] Machine.Start synchronously waits until a machine is runnig atleast thats how it is implemented for now in the VZ backend. But thats not how Stop works, it's just a signal at the moment and doesn't wait for the underlying machine state. This means in the stop command there's some ugly polling logic. Maybe it's nicer if neither start and stop wait and are just triggers, and I expose a WatchState function from the Machine api.
- [ ] Codesign doesn't work when running cargo build --release
- [ ] Error propagation doesn't work correctly. For example a non signed binary can't start VZ, but that error isn't correctly getting propaged to the cli. CLI just keeps waiting even though instanced has shutdown already. Also I would expect an erro log in the trace log and that doesn't exist either.
  - [ ] basically any error that happens in instanced doesn't propoagate to the cli and bento start just keeps hanging.
- [ ] Wrong guest-agent config crashes intsanced, but the start command will still wait until it timesout with no clear error message.
- [ ] Revisit extension architecture
- [ ] Implement metrics / logging endpoint

- [ ] If create fails due to intiramfs or kernel not existing at the particular destination it still creats all the folders that make ls think or subsequent creates think that the vm exists already. This should happen in some kind of transaction. Either the vm can be createa nd the folder exists or it fails and no files or written to disk!

## Packages needed to build the kernel in arch

- base-devel
- bc

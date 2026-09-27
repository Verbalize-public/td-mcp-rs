# Lifecycle and files

Current-tool procedure. Use {{ skill("operate") }} and {{ skill("recover") }}. Discovery/addressability is not permission to stop a user's process.

## Existing process or new process?

Reuse the intended authorized process when it is suitable. Create a task-owned process when the user requested a new project or isolation is actually needed—not automatically for every small read or change. Record the returned PID, daemon and project identity; do not infer ownership from a fleet row alone.

For a new project at a specifically authorized destination:

```json
{"projectPath":"/authorized/scratch/new.toe","createIfMissing":true}
```

This is a `spawn_td` argument example, not permission to use that literal path. `createIfMissing` is for an intended new target; it is not the fix for a mistyped existing-project path. The template may be user-customized: inspect the initial contents rather than assume a blank project. `templatePath`, installation selection and timeout controls must follow the current schema.

## Read the launch outcome

- `ok:true` with handshake: reconcile actual PID/project and proceed within scope.
- `wait_timeout`: inspect that process's state and bridge status before any respawn. A timeout does not prove process death.
- `exited_early`: use exit/launch/install/runner evidence. An empty dialog list on an unsupported backend does not prove absence of a modal.
- Use `dialogs` only when supported and relevant. Read a warning before choosing an authorized dismissal; do not accept license/build warnings blindly. On Linux/Wine, do not loop on a nonexistent dialogs backend.

Do not change global display/runner settings or stop a human-opened TD merely to make a probe pass. A controlled canary may diagnose the environment; one failed render or sandboxed command is not proof the driver or all TD is broken.

## Filesystem and persistence

The agent shell, daemon, remote host and TD/Wine may not see the same paths. Distinguish an authorized namespace/visibility correction from a policy denial. A denial cannot be bypassed by asking a host daemon to write the same protected file.

Separate:

- live scene changes;
- offline `.toe/.tox` expansion/packing;
- saving the actual current project/component;
- exporting a portable dependency package;
- fresh-load verification.

Use the correct native save surface; root-COMP save failure is not proof project saving is impossible. Do not turn a failed live save into copying an older on-disk project and call it a checkpoint. Preserve original files; replace only with authorized policy and verified recovery.

Save when the requested/implied deliverable requires it, to the authorized target. Otherwise disclose unsaved live changes rather than silently overwriting the user's project. Check the actual saved filename, bytes and relevant identity; a stale fleet title is not post-save proof. A reusable/exported deliverable needs its relevant reload checks; a read-only explanation does not.

## Cleanup and handoff

Stop only task-owned processes or explicitly authorized others, using the supported graceful path first. Force termination requires authority for losing unsaved work; it is not routine timeout recovery. Respect unknown/reused-PID checks. Report retained process, project/artifact locations, final transport state and remaining verification via {{ skill("verify-handoff") }}.

## Related

- {{ skill("operate") }}
- {{ skill("lifecycle") }}
- {{ skill("project-io") }}
- {{ skill("popups") }}
- {{ skill("verify-handoff") }}

**Canonical:** {{ skill("lifecycle-files") }}

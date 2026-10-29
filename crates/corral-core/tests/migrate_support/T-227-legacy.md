---
schema_version: 2
task:
  origin: pm
  result:
    text: 'Goal: give dispatched workers the complete procedure from dispatch to corral done in the corral-worker skill, without redefining the authoritative run.md contract. Achieved across R1 and round 1 rework: the skill carries the lifecycle walkthrough, the five kind variants (code change, spike/research, docs, flaky-test fix, hotfix), the rework procedure, six cited failure modes and out-of-scope filing via corral-submit. Each lifecycle step now points to its numbered Run contract step and adds only supplementary procedure, with no restated or stricter rules. It installs through the T-226 packaging, and the gate''s docs checker fails on any corral command or flag in a role skill that is missing from --help. Limitations: non-duplication was checked by a manual audit, not automatically; the checker validates flags per top-level command, not per nested subcommand; and the failure-mode citations are corral-project records.'
    run: R1
    spec_digest: 494942b85bd16fb8d96c3f0ca3f85e904e898376090556cb0ac0a883e6dffec5
  id: T-227
  title: 'Worker skill: complete procedure from dispatch to corral done'
  status: done
  project: corral
  kind: docs
  size: m
  priority: P1
  depends_on:
  - T-226
  rounds: 1
  created: 2026-10-20T00:16:31.496809298Z
  updated: 2026-10-20T07:07:20.875858396Z
  runs:
  - execution:
      resolved:
        profile: opus-med
        harness: omo
        command: omo -n '{name}' {model_args} "$(cat '{prompt_file}')"
        model: anthropic-subscription/claude-opus-5-5
        vendor: anthropic
        effort: medium
        model_args:
        - --model
        - anthropic-subscription/claude-opus-5-5
        - --thinking
        - medium
      revision: null
      evidence: null
      instructions: ''
      prompt_template: |
        # {{id}} {{title}}

        {{body}}

        Project: {{project}} at {{worktree}}; branch {{branch}} from {{base}}.

        ## Run contract

        You are the dispatched coding agent, not the workspace PM. PM board-event subscriptions and
        `[corral]` push messages coordinate the board; they do not replace this run's checks or completion report.

        Search `{{corral}} search "<query>" --in all --json` before re-running expensive investigations.

        1. Read {{agents_md}} and any deeper AGENTS.md before editing. Work only inside {{worktree}}; commit only
           on `{{branch}}`.
        2. Run the full gate in the foreground before `done`. Never end your turn waiting on a background job,
           and never run a command that reads the terminal (give anything that might read stdin `</dev/null`).
           {{gate}}
        3. Attach what proves each acceptance criterion (test output, live-check logs, screenshots; the gate log
           as `log`), with live checks in the isolated environment the repo names:

               {{artifact_cmd}}

           Files matching {{artifact_globs}} in the worktree are collected at `done` as well.
           Tag reports and summaries as `report`, raw command output as `log`, and supporting files as `evidence`
           (images as `screenshot`). Reports, summaries and pinned artifacts are protected from retention; bulky logs
           are eligible for the project's configured compression or purge policy. Use `corral artifact set-tier`
           with an artifact ref to override inferred importance when necessary.
        4. Before `done`: commit, rebase onto the current `{{base}}`, and re-run the gate on the rebased tree.
        5. Finish with:

               {{done_cmd}}

           Result shape: explain the original task goal, the cumulative achieved scope across all runs, and
           remaining limitations. Do not report only this run's last action. Keep this outcome separate from
           the activity summary and verification evidence. On a rebase-only resubmission, omit `--result` to
           preserve the existing outcome; update it when achieved scope or limitations change. A preserved
           result never means earlier checks remain valid: re-run checks on the current tree.

           Summary shape: this run's activity (one paragraph); then a line starting `Verified:` listing each command,
           test and live check you ran with its result; then a line starting `Not verified:` (or `Not verified:
           none`).
        6. If you need a decision, ask it plainly in this pane and stop; the board shows the run as blocked. Do
           not guess on a scope change.
    delivery: legacy
    inbox:
      next_seq: 1
      next_envelope: 1
      open_envelope: null
      effects_applied: []
      binding: null
      observe_cursor:
        offset: 0
        last_complete_line: 0
        coverage: complete
    completion:
      required_checks:
      - bash -c 'source scripts/test-herdr.sh && corral_test_herdr_start && scripts/gate.sh'
      audit:
        version: 1
        audited: false
        passed: true
        identity:
          version: 2
          task: T-227
          run: R1
          attempt: 90411-1792471678334606505
          head: add53e0d82c529944cde1442a3d193e1ae6437fa
          tree: b98fb9dd5ba76cce96a77dfb0608e88cd37a3fcc
          integration_base: e833a353c1d91d47be2f4f7ad746c88bbac9b17f
          dispatch_base: 438fb8db83d1cff947dc7251840a5d53bdc0aa82
          spec_digest: 494942b85bd16fb8d96c3f0ca3f85e904e898376090556cb0ac0a883e6dffec5
          policy_digest: 7fbc891278f2beaeecdd726ee625e4d17c010fe781736909ce65936cf4094725
          inputs_digest: 5eca3fc3394ad294feffb42929fa33807777d20bad146defaca3a8824adcb921
          environment:
          - - CARGO_HOME
            - null
          - - CARGO_TARGET_DIR
            - null
          - - CORRAL_HERDR_SESSION
            - null
          - - CORRAL_SIZE_LIMITS
            - '{"rs":800}'
          - - PATH
            - /home/pawis/.bun/install/global/node_modules/omo-ai/node_modules/.bin:/home/pawis/.local/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.grok/bin:/home/pawis/.bun/bin:/home/pawis/.local/bin:/home/pawis/.opencode/bin:/home/pawis/.local/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.grok/bin:/home/pawis/.bun/bin:/home/pawis/.deno/bin:/home/pawis/.local/bin:/home/pawis/.opencode/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games:/usr/lib/wsl/lib:/mnt/c/Program Files/PowerShell/7:/mnt/c/Program Files (x86)/Razer Chroma SDK/bin:/mnt/c/Program Files/Razer Chroma SDK/bin:/mnt/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.1/bin/x64:/mnt/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.1/bin:/mnt/c/Windows/system32:/mnt/c/Windows:/mnt/c/Windows/System32/Wbem:/mnt/c/Windows/System32/WindowsPowerShell/v1.0/:/mnt/c/Windows/System32/OpenSSH/:/mnt/c/Program Files/NVIDIA Corporation/NVIDIA App/NvDLISR:/mnt/c/Program Files (x86)/Windows Kits/10/Windows Performance Toolkit/:/mnt/c/Program Files (x86)/NVIDIA Corporation/PhysX/Common:/mnt/c/Program Files/NVIDIA Corporation/Nsight Compute 2025.4.1/:/mnt/c/Program Files/GitHub CLI/:/mnt/c/Program Files/dotnet/:/mnt/c/Program Files/nodejs/:/mnt/c/Windows/system32/config/systemprofile/AppData/Local/Microsoft/WindowsApps:/mnt/c/Windows/system32/config/systemprofile/.dotnet/tools:/mnt/c/Program Files (x86)/Razer/ChromaBroadcast/bin:/mnt/c/Program Files/Razer/ChromaBroadcast/bin:/mnt/c/Users/pawis/AppData/Local/Programs/OpenAI/Codex/bin:/mnt/c/Users/pawis/AppData/Local/hermes/hermes-agent/venv/Scripts:/mnt/c/Users/pawis/AppData/Local/hermes/bin:/mnt/c/Users/pawis/scoop/persist/bun/bin:/mnt/c/Users/pawis/scoop/apps/git/current/cmd:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/site/bin:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/bin:/mnt/c/Users/pawis/scoop/apps/mingw/current/bin:/mnt/c/Users/pawis/scoop/apps/python/current/Scripts:/mnt/c/Users/pawis/scoop/apps/python/current:/mnt/c/Users/pawis/.local/bin:/mnt/c/Users/pawis/.cargo/bin:/mnt/c/Users/pawis/scoop/shims:/mnt/c/Users/pawis/AppData/Local/Microsoft/WindowsApps:/mnt/c/Users/pawis/AppData/Local/Programs/Microsoft VS Code/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Podman/:/mnt/c/Users/pawis/AppData/Local/Programs/Antigravity/bin:/mnt/c/Users/pawis/AppData/Local/Programs/MiKTeX/miktex/bin/x64/:/mnt/c/Users/pawis/.lmstudio/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Ollama:/mnt/c/Users/pawis/AppData/Roaming/npm:/mnt/c/Users/pawis/.dotnet/tools:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe/ripgrep-15.1.0-x86_64-pc-windows-msvc:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin:/mnt/c/Program Files/PowerShell/7/:/mnt/c/Program Files/Git/cmd:/mnt/c/Users/pawis/AppData/Local/hermes/hermes-agent/bin:/mnt/c/Users/pawis/.grok/bin:/mnt/c/Users/pawis/AppData/Local/Programs/OpenAI/Codex/bin:/mnt/c/Users/pawis/AppData/Local/hermes/bin:/mnt/c/Users/pawis/scoop/persist/bun/bin:/mnt/c/Users/pawis/scoop/apps/git/current/cmd:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/site/bin:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/bin:/mnt/c/Users/pawis/scoop/apps/mingw/current/bin:/mnt/c/Users/pawis/scoop/apps/python/current/Scripts:/mnt/c/Users/pawis/scoop/apps/python/current:/mnt/c/Users/pawis/.local/bin:/mnt/c/Users/pawis/.cargo/bin:/mnt/c/Users/pawis/scoop/shims:/mnt/c/Users/pawis/AppData/Local/Microsoft/WindowsApps:/mnt/c/Users/pawis/AppData/Local/Programs/Microsoft VS Code/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Podman/:/mnt/c/Users/pawis/AppData/Local/Programs/Antigravity/bin:/mnt/c/Users/pawis/AppData/Local/Programs/MiKTeX/miktex/bin/x64/:/mnt/c/Users/pawis/.lmstudio/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Ollama:/mnt/c/Users/pawis/AppData/Roaming/npm:/mnt/c/Users/pawis/.dotnet/tools:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe/ripgrep-15.1.0-x86_64-pc-windows-msvc:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Cua/cua-driver/bin:/snap/bin:/usr/local/bin:/home/pawis/.local/bin:/usr/local/bin:/home/pawis/.local/bin
          - - RUSTC
            - null
          - - RUSTC_WRAPPER
            - null
          - - RUSTFLAGS
            - null
          - - RUSTUP_HOME
            - null
          - - RUSTUP_TOOLCHAIN
            - null
        manifest_digest: null
        required_checks: []
        evidence_keys: []
        failures: []
        checked: 2026-10-20T05:49:11.084897647Z
    id: R1
    progress:
      revision: 2
      spec_revision: 7094d9e20386f1fe9b04ce10d22e353d56de9186bf1ecd63e045795131549d1f
      phases:
      - id: investigation
        label: Investigation
        steps:
        - id: read-contract
          label: Read AGENTS.md, run contract, T-226 packaging
          status:
            state: done
          criteria: []
        - id: lessons
          label: Collect lessons and run history citations
          status:
            state: done
          criteria: []
      - id: implementation
        label: Implementation
        steps:
        - id: skill-body
          label: Write corral-worker skill body with variants
          status:
            state: done
          criteria: []
        - id: verb-check
          label: Add verb-validity check against corral help
          status:
            state: done
          criteria: []
      - id: verification
        label: Verification
        steps:
        - id: gate
          label: Run verb check and gate on rebased tree
          status:
            state: done
          criteria: []
        - id: done
          label: Attach artifacts and run corral done
          status:
            state: done
          criteria: []
      - id: rework-1
        label: Round 1 rework
        steps:
        - id: dedupe-contract
          label: Replace run.md restatements with contract pointers
          status:
            state: done
          criteria: []
        - id: regate-r2
          label: Rebase on main and re-run gate
          status:
            state: done
          criteria: []
    checkpoints:
    - checkpoint:
        version: 1
        revision: 1
        input:
          task: T-227
          run: R1
          spec_revision: 7094d9e20386f1fe9b04ce10d22e353d56de9186bf1ecd63e045795131549d1f
          expected_revision: 0
          progress_revision: 1
          head: 2c5c87ce76b7b6cf863f7bf19eb8bf538b34780b
          base: 438fb8db83d1cff947dc7251840a5d53bdc0aa82
          tree: 4736e833821b730c5565bcc25521eabad39dcdfb9b956b1bd26ce7676ae79c18
          criterion_ids: []
          evidence_refs: []
          next_action: Run corral done T-227 with the prepared summary and result
          decisions:
          - Validate role-skill verbs by extending scripts/check-docs.sh, which already runs in the gate
          waits: []
          resources: []
        progress:
          revision: 1
          spec_revision: 7094d9e20386f1fe9b04ce10d22e353d56de9186bf1ecd63e045795131549d1f
          phases:
          - id: investigation
            label: Investigation
            steps:
            - id: read-contract
              label: Read AGENTS.md, run contract, T-226 packaging
              status:
                state: done
              criteria: []
            - id: lessons
              label: Collect lessons and run history citations
              status:
                state: done
              criteria: []
          - id: implementation
            label: Implementation
            steps:
            - id: skill-body
              label: Write corral-worker skill body with variants
              status:
                state: done
              criteria: []
            - id: verb-check
              label: Add verb-validity check against corral help
              status:
                state: done
              criteria: []
          - id: verification
            label: Verification
            steps:
            - id: gate
              label: Run verb check and gate on rebased tree
              status:
                state: done
              criteria: []
            - id: done
              label: Attach artifacts and run corral done
              status:
                state: in_progress
              criteria: []
        completed_steps:
        - read-contract
        - lessons
        - skill-body
        - verb-check
        - gate
        current_step: done
        captured: 2026-10-20T04:08:28.754043617Z
      artifact: artifacts/T-227/R1/d4e28926fb388f598223957abfe5707e0363efddbbc8cfb462b209d7cf52044f-checkpoint-1.json
    - checkpoint:
        version: 1
        revision: 2
        input:
          task: T-227
          run: R1
          spec_revision: 7094d9e20386f1fe9b04ce10d22e353d56de9186bf1ecd63e045795131549d1f
          expected_revision: 1
          progress_revision: 2
          head: add53e0d82c529944cde1442a3d193e1ae6437fa
          base: e833a353c1d91d47be2f4f7ad746c88bbac9b17f
          tree: 7dee209bc8c8fc76896476cae534ca6d67cde3c7be48169464d1d62f67d9d4f1
          criterion_ids: []
          evidence_refs: []
          next_action: Resubmit T-227 with corral done after round 1 rework
          decisions:
          - Skill steps point at run.md steps instead of restating them
          waits: []
          resources: []
        progress:
          revision: 2
          spec_revision: 7094d9e20386f1fe9b04ce10d22e353d56de9186bf1ecd63e045795131549d1f
          phases:
          - id: investigation
            label: Investigation
            steps:
            - id: read-contract
              label: Read AGENTS.md, run contract, T-226 packaging
              status:
                state: done
              criteria: []
            - id: lessons
              label: Collect lessons and run history citations
              status:
                state: done
              criteria: []
          - id: implementation
            label: Implementation
            steps:
            - id: skill-body
              label: Write corral-worker skill body with variants
              status:
                state: done
              criteria: []
            - id: verb-check
              label: Add verb-validity check against corral help
              status:
                state: done
              criteria: []
          - id: verification
            label: Verification
            steps:
            - id: gate
              label: Run verb check and gate on rebased tree
              status:
                state: done
              criteria: []
            - id: done
              label: Attach artifacts and run corral done
              status:
                state: done
              criteria: []
          - id: rework-1
            label: Round 1 rework
            steps:
            - id: dedupe-contract
              label: Replace run.md restatements with contract pointers
              status:
                state: done
              criteria: []
            - id: regate-r2
              label: Rebase on main and re-run gate
              status:
                state: done
              criteria: []
        completed_steps:
        - read-contract
        - lessons
        - skill-body
        - verb-check
        - gate
        - done
        - dedupe-contract
        - regate-r2
        current_step: null
        captured: 2026-10-20T05:49:08.407704985Z
      artifact: artifacts/T-227/R1/b2bb56af1a20a6b30c5590c5bdbbf9fe25b04f02dba94cc1049613fcab15fded-checkpoint-2.json
    checkpoint_required: true
    agent: opus-med
    harness: omo
    model: anthropic-subscription/claude-opus-5-5
    effort: medium
    route:
      source: rule
      index: 4
      name: 'Size spread m: opus medium'
    vendor: anthropic
    failures: 1
    mode: interactive
    worktree: /home/pawis/miti-workspace/corral/.worktrees/T-227
    branch: corral/T-227
    base: main
    base_sha: 438fb8db83d1cff947dc7251840a5d53bdc0aa82
    herdr:
      workspace_id: w8V
      pane_id: w8V:p1
      socket: /home/pawis/.config/herdr/herdr.sock
    started: 2026-10-20T03:55:34.257003974Z
    ended: 2026-10-20T05:49:13.615920599Z
    outcome: done
    summary: |-
      Round 1 rework for the gpt-sol P2 finding. The worker skill restated run.md rules: the foreground gate, artifact kind classification and collection at done, commit/rebase/re-gate, the summary shape, ask-then-stop, rebase-only --result handling, and the stale-checkpoint and progress-is-not-acceptance rules from the Portable resume contract. It also added a stricter one-artifact-per-criterion rule. I replaced each restatement with a pointer to the owning contract step, removed the one-artifact-per-criterion rule, and kept only supplementary procedure: the pipefail logging tip, artifact titles and listing, set-tier for a retained proof log, the rebase command, a separate post-rebase log, conflict handling, the done --check preflight, harness-neutral reply delivery, run decision/run wait/run resume, and how to locate review notes. The spike, hotfix and rework sections now cite contract steps 2 to 5 instead of paraphrasing them. I audited the whole skill against run.md paragraph by paragraph (round-1 rework report). Committed as add53e0 and rebased onto main e833a35.
      Verified: scripts/check-docs.sh target/debug/corral passes on the reworked tree (30 files including 3 role skills, exit 0); the full gate on the rebased tree passed (fmt, 800-SLOC ceiling, clippy -D warnings, cargo test --workspace in isolated corral-test herdr with 2706 passed, 0 failed and 2 ignored, build plus check-docs; gate-r2.log); progress and checkpoint saved from fresh readback.
      Not verified: the run.md non-duplication comparison is a manual audit (tabulated in the report), not an automated check; I did not re-run the R1 negative verb-check probe because the checker is unchanged since R1; no dispatched worker has used the skill yet.
    agent_state: done
    seen_pane: true
    seen_working: true
    gate:
      identity:
        version: 2
        task: T-227
        run: R1
        attempt: 1731776-1792478412711182080
        head: add53e0d82c529944cde1442a3d193e1ae6437fa
        tree: b98fb9dd5ba76cce96a77dfb0608e88cd37a3fcc
        integration_base: e833a353c1d91d47be2f4f7ad746c88bbac9b17f
        dispatch_base: 438fb8db83d1cff947dc7251840a5d53bdc0aa82
        spec_digest: 494942b85bd16fb8d96c3f0ca3f85e904e898376090556cb0ac0a883e6dffec5
        policy_digest: 7fbc891278f2beaeecdd726ee625e4d17c010fe781736909ce65936cf4094725
        inputs_digest: 1e4a6c744fa6ac2c35b7a6c932be43428163ac0a4e8738eae05db50b6057af9b
        environment:
        - - CARGO_HOME
          - null
        - - CARGO_TARGET_DIR
          - null
        - - CORRAL_HERDR_SESSION
          - null
        - - CORRAL_SIZE_LIMITS
          - '{"rs":800}'
        - - PATH
          - /home/pawis/.local/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.grok/bin:/home/pawis/.bun/bin:/home/pawis/.deno/bin:/home/pawis/.local/bin:/home/pawis/.opencode/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games:/usr/lib/wsl/lib:/mnt/c/Program Files/PowerShell/7:/mnt/c/Program Files (x86)/Razer Chroma SDK/bin:/mnt/c/Program Files/Razer Chroma SDK/bin:/mnt/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.1/bin/x64:/mnt/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.1/bin:/mnt/c/Windows/system32:/mnt/c/Windows:/mnt/c/Windows/System32/Wbem:/mnt/c/Windows/System32/WindowsPowerShell/v1.0/:/mnt/c/Windows/System32/OpenSSH/:/mnt/c/Program Files/NVIDIA Corporation/NVIDIA App/NvDLISR:/mnt/c/Program Files (x86)/Windows Kits/10/Windows Performance Toolkit/:/mnt/c/Program Files (x86)/NVIDIA Corporation/PhysX/Common:/mnt/c/Program Files/NVIDIA Corporation/Nsight Compute 2025.4.1/:/mnt/c/Program Files/GitHub CLI/:/mnt/c/Program Files/dotnet/:/mnt/c/Program Files/nodejs/:/mnt/c/Windows/system32/config/systemprofile/AppData/Local/Microsoft/WindowsApps:/mnt/c/Windows/system32/config/systemprofile/.dotnet/tools:/mnt/c/Program Files (x86)/Razer/ChromaBroadcast/bin:/mnt/c/Program Files/Razer/ChromaBroadcast/bin:/mnt/c/Users/pawis/AppData/Local/Programs/OpenAI/Codex/bin:/mnt/c/Users/pawis/AppData/Local/hermes/hermes-agent/venv/Scripts:/mnt/c/Users/pawis/AppData/Local/hermes/bin:/mnt/c/Users/pawis/scoop/persist/bun/bin:/mnt/c/Users/pawis/scoop/apps/git/current/cmd:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/site/bin:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/bin:/mnt/c/Users/pawis/scoop/apps/mingw/current/bin:/mnt/c/Users/pawis/scoop/apps/python/current/Scripts:/mnt/c/Users/pawis/scoop/apps/python/current:/mnt/c/Users/pawis/.local/bin:/mnt/c/Users/pawis/.cargo/bin:/mnt/c/Users/pawis/scoop/shims:/mnt/c/Users/pawis/AppData/Local/Microsoft/WindowsApps:/mnt/c/Users/pawis/AppData/Local/Programs/Microsoft VS Code/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Podman/:/mnt/c/Users/pawis/AppData/Local/Programs/Antigravity/bin:/mnt/c/Users/pawis/AppData/Local/Programs/MiKTeX/miktex/bin/x64/:/mnt/c/Users/pawis/.lmstudio/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Ollama:/mnt/c/Users/pawis/AppData/Roaming/npm:/mnt/c/Users/pawis/.dotnet/tools:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe/ripgrep-15.1.0-x86_64-pc-windows-msvc:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin:/mnt/c/Program Files/PowerShell/7/:/mnt/c/Program Files/Git/cmd:/mnt/c/Users/pawis/AppData/Local/hermes/hermes-agent/bin:/mnt/c/Users/pawis/.grok/bin:/mnt/c/Users/pawis/AppData/Local/Programs/OpenAI/Codex/bin:/mnt/c/Users/pawis/AppData/Local/hermes/bin:/mnt/c/Users/pawis/scoop/persist/bun/bin:/mnt/c/Users/pawis/scoop/apps/git/current/cmd:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/site/bin:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/bin:/mnt/c/Users/pawis/scoop/apps/mingw/current/bin:/mnt/c/Users/pawis/scoop/apps/python/current/Scripts:/mnt/c/Users/pawis/scoop/apps/python/current:/mnt/c/Users/pawis/.local/bin:/mnt/c/Users/pawis/.cargo/bin:/mnt/c/Users/pawis/scoop/shims:/mnt/c/Users/pawis/AppData/Local/Microsoft/WindowsApps:/mnt/c/Users/pawis/AppData/Local/Programs/Microsoft VS Code/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Podman/:/mnt/c/Users/pawis/AppData/Local/Programs/Antigravity/bin:/mnt/c/Users/pawis/AppData/Local/Programs/MiKTeX/miktex/bin/x64/:/mnt/c/Users/pawis/.lmstudio/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Ollama:/mnt/c/Users/pawis/AppData/Roaming/npm:/mnt/c/Users/pawis/.dotnet/tools:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe/ripgrep-15.1.0-x86_64-pc-windows-msvc:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Cua/cua-driver/bin:/snap/bin:/usr/local/bin:/home/pawis/.local/bin
        - - RUSTC
          - null
        - - RUSTC_WRAPPER
          - null
        - - RUSTFLAGS
          - null
        - - RUSTUP_HOME
          - null
        - - RUSTUP_TOOLCHAIN
          - null
      commands:
      - command: bash -c 'source scripts/test-herdr.sh && corral_test_herdr_start && scripts/gate.sh'
        exit_code: 0
        log_start: 0
        log_end: 245754
      log_sha256: 9cf2906b4b620023787598945856e68d87b784c97dd8a205bc104e348da52926
      result: pass
      at: 2026-10-20T06:46:34.409744981Z
      sha: add53e0d82c529944cde1442a3d193e1ae6437fa
      pane: w8V:p5
      socket: /home/pawis/.config/herdr/herdr.sock
    review:
      identity:
        version: 2
        task: T-227
        run: R1
        attempt: 1731776-1792478412711182080
        head: add53e0d82c529944cde1442a3d193e1ae6437fa
        tree: b98fb9dd5ba76cce96a77dfb0608e88cd37a3fcc
        integration_base: e833a353c1d91d47be2f4f7ad746c88bbac9b17f
        dispatch_base: 438fb8db83d1cff947dc7251840a5d53bdc0aa82
        spec_digest: 494942b85bd16fb8d96c3f0ca3f85e904e898376090556cb0ac0a883e6dffec5
        policy_digest: 7fbc891278f2beaeecdd726ee625e4d17c010fe781736909ce65936cf4094725
        inputs_digest: 1e4a6c744fa6ac2c35b7a6c932be43428163ac0a4e8738eae05db50b6057af9b
        environment:
        - - CARGO_HOME
          - null
        - - CARGO_TARGET_DIR
          - null
        - - CORRAL_HERDR_SESSION
          - null
        - - CORRAL_SIZE_LIMITS
          - '{"rs":800}'
        - - PATH
          - /home/pawis/.local/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.grok/bin:/home/pawis/.bun/bin:/home/pawis/.deno/bin:/home/pawis/.local/bin:/home/pawis/.opencode/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games:/usr/lib/wsl/lib:/mnt/c/Program Files/PowerShell/7:/mnt/c/Program Files (x86)/Razer Chroma SDK/bin:/mnt/c/Program Files/Razer Chroma SDK/bin:/mnt/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.1/bin/x64:/mnt/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.1/bin:/mnt/c/Windows/system32:/mnt/c/Windows:/mnt/c/Windows/System32/Wbem:/mnt/c/Windows/System32/WindowsPowerShell/v1.0/:/mnt/c/Windows/System32/OpenSSH/:/mnt/c/Program Files/NVIDIA Corporation/NVIDIA App/NvDLISR:/mnt/c/Program Files (x86)/Windows Kits/10/Windows Performance Toolkit/:/mnt/c/Program Files (x86)/NVIDIA Corporation/PhysX/Common:/mnt/c/Program Files/NVIDIA Corporation/Nsight Compute 2025.4.1/:/mnt/c/Program Files/GitHub CLI/:/mnt/c/Program Files/dotnet/:/mnt/c/Program Files/nodejs/:/mnt/c/Windows/system32/config/systemprofile/AppData/Local/Microsoft/WindowsApps:/mnt/c/Windows/system32/config/systemprofile/.dotnet/tools:/mnt/c/Program Files (x86)/Razer/ChromaBroadcast/bin:/mnt/c/Program Files/Razer/ChromaBroadcast/bin:/mnt/c/Users/pawis/AppData/Local/Programs/OpenAI/Codex/bin:/mnt/c/Users/pawis/AppData/Local/hermes/hermes-agent/venv/Scripts:/mnt/c/Users/pawis/AppData/Local/hermes/bin:/mnt/c/Users/pawis/scoop/persist/bun/bin:/mnt/c/Users/pawis/scoop/apps/git/current/cmd:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/site/bin:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/bin:/mnt/c/Users/pawis/scoop/apps/mingw/current/bin:/mnt/c/Users/pawis/scoop/apps/python/current/Scripts:/mnt/c/Users/pawis/scoop/apps/python/current:/mnt/c/Users/pawis/.local/bin:/mnt/c/Users/pawis/.cargo/bin:/mnt/c/Users/pawis/scoop/shims:/mnt/c/Users/pawis/AppData/Local/Microsoft/WindowsApps:/mnt/c/Users/pawis/AppData/Local/Programs/Microsoft VS Code/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Podman/:/mnt/c/Users/pawis/AppData/Local/Programs/Antigravity/bin:/mnt/c/Users/pawis/AppData/Local/Programs/MiKTeX/miktex/bin/x64/:/mnt/c/Users/pawis/.lmstudio/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Ollama:/mnt/c/Users/pawis/AppData/Roaming/npm:/mnt/c/Users/pawis/.dotnet/tools:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe/ripgrep-15.1.0-x86_64-pc-windows-msvc:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin:/mnt/c/Program Files/PowerShell/7/:/mnt/c/Program Files/Git/cmd:/mnt/c/Users/pawis/AppData/Local/hermes/hermes-agent/bin:/mnt/c/Users/pawis/.grok/bin:/mnt/c/Users/pawis/AppData/Local/Programs/OpenAI/Codex/bin:/mnt/c/Users/pawis/AppData/Local/hermes/bin:/mnt/c/Users/pawis/scoop/persist/bun/bin:/mnt/c/Users/pawis/scoop/apps/git/current/cmd:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/site/bin:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/bin:/mnt/c/Users/pawis/scoop/apps/mingw/current/bin:/mnt/c/Users/pawis/scoop/apps/python/current/Scripts:/mnt/c/Users/pawis/scoop/apps/python/current:/mnt/c/Users/pawis/.local/bin:/mnt/c/Users/pawis/.cargo/bin:/mnt/c/Users/pawis/scoop/shims:/mnt/c/Users/pawis/AppData/Local/Microsoft/WindowsApps:/mnt/c/Users/pawis/AppData/Local/Programs/Microsoft VS Code/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Podman/:/mnt/c/Users/pawis/AppData/Local/Programs/Antigravity/bin:/mnt/c/Users/pawis/AppData/Local/Programs/MiKTeX/miktex/bin/x64/:/mnt/c/Users/pawis/.lmstudio/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Ollama:/mnt/c/Users/pawis/AppData/Roaming/npm:/mnt/c/Users/pawis/.dotnet/tools:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe/ripgrep-15.1.0-x86_64-pc-windows-msvc:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Cua/cua-driver/bin:/snap/bin:/usr/local/bin:/home/pawis/.local/bin
        - - RUSTC
          - null
        - - RUSTC_WRAPPER
          - null
        - - RUSTFLAGS
          - null
        - - RUSTUP_HOME
          - null
        - - RUSTUP_TOOLCHAIN
          - null
      evidence_digest: 4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945
      verdict: approve
      reviewer: gpt-sol
      notes: |
        # T-227 R1 independent re-review

        Verdict: approve. No blocking findings.

        Reviewed attempt `1731776-1792478412711182080`, HEAD
        `add53e0d82c529944cde1442a3d193e1ae6437fa`, tree
        `b98fb9dd5ba76cce96a77dfb0608e88cd37a3fcc`, integrated base
        `e833a353c1d91d47be2f4f7ad746c88bbac9b17f`.

        ## Findings and previous required change

        The prior P2 finding is resolved. Compared the complete revised skill with
        `crates/corral-ops/assets/run.md`, the Portable resume guidance in
        `docs/guide/run-progress.md`, and the prior reviewer notes
        `artifacts/T-227/R1/a356d46a-review-notes.md`.

        The lifecycle now points to contract steps 1-6 rather than rewording their
        rules. It retains supplementary mechanics: progress payload readback,
        pipefail logging, artifact listing and retention override, the rebase command,
        separate post-rebase evidence, conflict handling, completion preflight,
        harness-neutral replies, durable decisions/waits, and locating review notes.
        The one-artifact-per-criterion requirement is gone. Rework delegates
        rebase-only result preservation to contract step 5. The kind-specific guidance
        and cited lessons supplement rather than redefine the contract.

        ## Acceptance and evidence mapping

        `corral criteria list T-227` returned `manifest: null` and spec digest
        `494942b85bd16fb8d96c3f0ca3f85e904e898376090556cb0ac0a883e6dffec5`.
        There are no registered criterion IDs or observations to map. The following
        maps every specification obligation to this integrated candidate instead.
        Worker reports, artifact titles, and Verified paragraphs were not treated as
        machine PASS evidence.

        | Requirement | Current-candidate evidence and observation | Assessment |
        |---|---|---|
        | Installed through T-226 packaging | Ran `target/debug/corral skill install --dir /tmp/corral-T-227-review-1731776`; exit 0, all four roles installed. Read the installed worker asset and compared it with the source after substituting the managed stamp for `{{marker}}`; exact equality. `crates/corral-ops/src/skill.rs:400` embeds this asset. | Proven on the installed surface. |
        | Every kind variant present | Read the entire source and checked the installed asset for Code change, Spike or research, Docs, Flaky-test fix, and Hotfix headings. Research specifies a report and no production changes. Flaky-test guidance requires deterministic events/barriers/clocks and forbids sleeps, expanded budgets, retries, and skipped tests. | Proven by shipped content inspection, not merely headings. |
        | Extract and validate shown commands | Independently ran `scripts/check-docs.sh target/debug/corral`; exit 0, `docs ok: 2048 code spans/blocks, 29 toml examples, 30 files`. The changed input glob includes all three role-skill assets. Read the checker and the documented command forms. | Proven for the commands shown in this candidate. |
        | Validation can fail | Executed the checker's actual Python functions in memory, without editing files, against `corral nosuchverb`, `corral run frobnicate`, and `corral done --no-such-flag`. Each produced exactly one expected rejection; all assertions passed. | Proven for invalid top-level verbs, nested verbs, and unknown flags. |
        | Contract authority and no differently worded duplicated rules | Manually compared every paragraph with current run.md. The skill uses numbered contract-step references for AGENTS.md, foreground gate, artifact classification/collection, commit/rebase/re-gate, result/summary shape, asking/stopping, and rebase-only result handling. The formerly stricter artifact-count rule is absent. | Met; semantic inspection, not an automated prose check. |
        | Lifecycle, progress, checkpoint, gate, artifacts, commit, rebase, done | Worker lines 17-39 cover the lifecycle through the contract pointers and supplementary commands. Step 1 delegates AGENTS.md to contract step 1; step 7 delegates the result and Verified/Not verified summary shapes to contract step 5. Artifact kinds and tiers remain owned by contract step 3, with listing/set-tier mechanics in the skill. | Complete without duplicating the contract. |
        | Questions, stopping, waits/decisions, harness-neutral delivery | Worker lines 40-47 reference contract step 6, describe delivery without harness-specific tools, and show decision, wait, and resume operations. Compared with `docs/guide/automation.md:571-642`. | Met. |
        | Rework after changes and rebase-only resubmission | Worker lines 82-94 explain live requests versus fresh retries, reading all notes, tracking and addressing findings, reviewing inherited work, and resubmitting through contract steps 2-5. The result decision points to step 5's owning rule. | Met. |
        | Cited failure modes from project lessons/history | Read the four cited lesson entries in `docs/records/T-225-lessons-proposal.json`; their evidence supports the green-gate, foreign-session variant, deterministic race-test, and conflict-invariant claims. Read T-045 report row E2 for T-026's 821-SLOC rejection, and automation.md's watchdog section. | Grounded in identifiable project records; each item has a citation. |
        | Out-of-scope follow-ups through submit skill | Worker lines 119-124 explicitly use `corral-submit`, show an agent-origin filing example, and prohibit expanding the diff for unrelated findings. | Met. |
        | Scope, conventions, tests | The full integrated diff changes only the worker asset, skills guide, and docs-check input glob. No Rust behavior, dependency, or test is changed; no test is weakened or suppressed. `git diff main...HEAD --check` exits 0. | Minimal and in scope. |

        ## Current-attempt gate and integration evidence

        Read and parsed
        `/home/pawis/miti-workspace/.corral/artifacts/T-227/R1/941e2a90-T-227-R1-1731776-1792478412711182080.json`.
        Its attempt, HEAD, tree, base, and spec digest match this review. It records
        `result: pass`, `exit_code: 0`, and exit 0 for:

        ```sh
        bash -c 'source scripts/test-herdr.sh && corral_test_herdr_start && scripts/gate.sh'
        ```

        The matching machine log
        `/home/pawis/miti-workspace/.corral/state/gates/T-227-R1-1731776-1792478412711182080.log`
        ends with the docs checker PASS at line 3505, `Gate passed.` at line 3507,
        and `corral gate: pass` at line 3509. It records two ignored tests, not new
        skips introduced by this documentation-only diff. I did not rerun the full
        workspace gate; I independently reran the affected docs checker and exercised
        installation and negative command validation.

        Read the current integration repair packet
        `artifacts/T-227/R1/f8dcacac-022232093e94e598817e040610372aea4b7a87621ad0f15f23f650eb2561544f.json`.
        It names the same HEAD/base, no missing base commits or conflicts, and exactly
        the three intended paths. Independently re-read the integrated candidate,
        not only a pre-rebase artifact. Final Git readback confirmed HEAD, main, and
        HEAD tree match the identities above.

        ## Limits and worktree preservation

        No dispatched-worker usability trial was performed; reduced time to completion
        is unmeasured, not a required acceptance gate for this content task. Contract
        non-duplication is a manual semantic assessment. The checker is not a general
        shell parser; the actual documented forms passed its checks and inspection.
        No unresolved semantic obligation or missing required live evidence remains.

        The candidate worktree was clean on entry and clean after verification.
        All review notes are outside it; installation used the temporary directory
        above, and negative probes ran in memory. No candidate file was edited.
      sha: add53e0d82c529944cde1442a3d193e1ae6437fa
      at: 2026-10-20T07:04:29.561351776Z
      pane: w8V:p6
    review_runs:
    - delivery: legacy
      inbox:
        next_seq: 1
        next_envelope: 1
        open_envelope: null
        effects_applied: []
        binding: null
        observe_cursor:
          offset: 0
          last_complete_line: 0
          coverage: complete
      completion: {}
      id: R1-review1
      agent: gpt-sol
      harness: omo
      model: chatgpt-subscription/gpt-6.1-sol
      effort: high
      vendor: openai
      mode: interactive
      worktree: /home/pawis/miti-workspace/corral/.worktrees/T-227
      branch: corral/T-227
      base: main
      base_sha: 438fb8db83d1cff947dc7251840a5d53bdc0aa82
      herdr:
        workspace_id: w8V
        pane_id: w8V:p4
        socket: /home/pawis/.config/herdr/herdr.sock
      started: 2026-10-20T05:18:23.673828422Z
      ended: 2026-10-20T05:22:13.059556047Z
      seen_working: false
      review:
        identity:
          version: 2
          task: T-227
          run: R1
          attempt: 90411-1792471678334606505
          head: 36dc281d9c563f41990424913d2b96a4040b03db
          tree: 829b4c312821cc417b39f7bf3277f53519df5c8b
          integration_base: d6819ae4222ce15bf58745ba87fb2394333d6bd8
          dispatch_base: 438fb8db83d1cff947dc7251840a5d53bdc0aa82
          spec_digest: 494942b85bd16fb8d96c3f0ca3f85e904e898376090556cb0ac0a883e6dffec5
          policy_digest: 7fbc891278f2beaeecdd726ee625e4d17c010fe781736909ce65936cf4094725
          inputs_digest: 5eca3fc3394ad294feffb42929fa33807777d20bad146defaca3a8824adcb921
          environment:
          - - CARGO_HOME
            - null
          - - CARGO_TARGET_DIR
            - null
          - - CORRAL_HERDR_SESSION
            - null
          - - CORRAL_SIZE_LIMITS
            - '{"rs":800}'
          - - PATH
            - /home/pawis/.bun/install/global/node_modules/omo-ai/node_modules/.bin:/home/pawis/.local/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.grok/bin:/home/pawis/.bun/bin:/home/pawis/.local/bin:/home/pawis/.opencode/bin:/home/pawis/.local/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.grok/bin:/home/pawis/.bun/bin:/home/pawis/.deno/bin:/home/pawis/.local/bin:/home/pawis/.opencode/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games:/usr/lib/wsl/lib:/mnt/c/Program Files/PowerShell/7:/mnt/c/Program Files (x86)/Razer Chroma SDK/bin:/mnt/c/Program Files/Razer Chroma SDK/bin:/mnt/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.1/bin/x64:/mnt/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.1/bin:/mnt/c/Windows/system32:/mnt/c/Windows:/mnt/c/Windows/System32/Wbem:/mnt/c/Windows/System32/WindowsPowerShell/v1.0/:/mnt/c/Windows/System32/OpenSSH/:/mnt/c/Program Files/NVIDIA Corporation/NVIDIA App/NvDLISR:/mnt/c/Program Files (x86)/Windows Kits/10/Windows Performance Toolkit/:/mnt/c/Program Files (x86)/NVIDIA Corporation/PhysX/Common:/mnt/c/Program Files/NVIDIA Corporation/Nsight Compute 2025.4.1/:/mnt/c/Program Files/GitHub CLI/:/mnt/c/Program Files/dotnet/:/mnt/c/Program Files/nodejs/:/mnt/c/Windows/system32/config/systemprofile/AppData/Local/Microsoft/WindowsApps:/mnt/c/Windows/system32/config/systemprofile/.dotnet/tools:/mnt/c/Program Files (x86)/Razer/ChromaBroadcast/bin:/mnt/c/Program Files/Razer/ChromaBroadcast/bin:/mnt/c/Users/pawis/AppData/Local/Programs/OpenAI/Codex/bin:/mnt/c/Users/pawis/AppData/Local/hermes/hermes-agent/venv/Scripts:/mnt/c/Users/pawis/AppData/Local/hermes/bin:/mnt/c/Users/pawis/scoop/persist/bun/bin:/mnt/c/Users/pawis/scoop/apps/git/current/cmd:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/site/bin:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/bin:/mnt/c/Users/pawis/scoop/apps/mingw/current/bin:/mnt/c/Users/pawis/scoop/apps/python/current/Scripts:/mnt/c/Users/pawis/scoop/apps/python/current:/mnt/c/Users/pawis/.local/bin:/mnt/c/Users/pawis/.cargo/bin:/mnt/c/Users/pawis/scoop/shims:/mnt/c/Users/pawis/AppData/Local/Microsoft/WindowsApps:/mnt/c/Users/pawis/AppData/Local/Programs/Microsoft VS Code/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Podman/:/mnt/c/Users/pawis/AppData/Local/Programs/Antigravity/bin:/mnt/c/Users/pawis/AppData/Local/Programs/MiKTeX/miktex/bin/x64/:/mnt/c/Users/pawis/.lmstudio/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Ollama:/mnt/c/Users/pawis/AppData/Roaming/npm:/mnt/c/Users/pawis/.dotnet/tools:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe/ripgrep-15.1.0-x86_64-pc-windows-msvc:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin:/mnt/c/Program Files/PowerShell/7/:/mnt/c/Program Files/Git/cmd:/mnt/c/Users/pawis/AppData/Local/hermes/hermes-agent/bin:/mnt/c/Users/pawis/.grok/bin:/mnt/c/Users/pawis/AppData/Local/Programs/OpenAI/Codex/bin:/mnt/c/Users/pawis/AppData/Local/hermes/bin:/mnt/c/Users/pawis/scoop/persist/bun/bin:/mnt/c/Users/pawis/scoop/apps/git/current/cmd:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/site/bin:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/bin:/mnt/c/Users/pawis/scoop/apps/mingw/current/bin:/mnt/c/Users/pawis/scoop/apps/python/current/Scripts:/mnt/c/Users/pawis/scoop/apps/python/current:/mnt/c/Users/pawis/.local/bin:/mnt/c/Users/pawis/.cargo/bin:/mnt/c/Users/pawis/scoop/shims:/mnt/c/Users/pawis/AppData/Local/Microsoft/WindowsApps:/mnt/c/Users/pawis/AppData/Local/Programs/Microsoft VS Code/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Podman/:/mnt/c/Users/pawis/AppData/Local/Programs/Antigravity/bin:/mnt/c/Users/pawis/AppData/Local/Programs/MiKTeX/miktex/bin/x64/:/mnt/c/Users/pawis/.lmstudio/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Ollama:/mnt/c/Users/pawis/AppData/Roaming/npm:/mnt/c/Users/pawis/.dotnet/tools:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe/ripgrep-15.1.0-x86_64-pc-windows-msvc:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Cua/cua-driver/bin:/snap/bin:/usr/local/bin:/home/pawis/.local/bin:/usr/local/bin:/home/pawis/.local/bin
          - - RUSTC
            - null
          - - RUSTC_WRAPPER
            - null
          - - RUSTFLAGS
            - null
          - - RUSTUP_HOME
            - null
          - - RUSTUP_TOOLCHAIN
            - null
        evidence_digest: 4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945
        verdict: changes
        reviewer: gpt-sol
        notes: |
          # T-227 R1 review

          Verdict: changes.

          Reviewed attempt `90411-1792471678334606505`, HEAD
          `36dc281d9c563f41990424913d2b96a4040b03db`, tree
          `829b4c312821cc417b39f7bf3277f53519df5c8b`, integrated base
          `d6819ae4222ce15bf58745ba87fb2394333d6bd8`.

          ## Required change

          ### P2: Keep contract rules in run.md instead of restating and strengthening them

          `crates/corral-ops/assets/skills/corral-worker.md:30-43` repeats rules already
          owned by `crates/corral-ops/assets/run.md` steps 2-4: foreground gating, artifact
          classification, collection at done, and committing/rebasing/re-gating. The
          rework paragraph at worker lines 103-105 also restates step 5's result
          preservation rule. Adding a contract-step reference before the restatement
          does not meet the explicit acceptance criterion, "No rule duplicated from
          run.md with different wording; the skill links to the contract instead."

          There is already a substantive divergence at worker line 33: "One artifact per
          criterion at minimum" imposes an artifact-count requirement. Contract step 3
          requires attaching proof of each criterion, but does not require separate
          artifacts; one log can prove multiple criteria.

          Replace the duplicated normative text with references to the relevant contract
          steps. Keep the useful supplementary procedure, such as pipefail when logging,
          artifact listing and retention override commands, separate post-rebase logs,
          conflict handling, completion preflight, and locating review notes. Remove the
          new one-artifact-per-criterion rule. Audit the whole skill for the same issue,
          including the summary and ask-and-stop paragraphs, while preserving the
          requested lifecycle and kind variants. Re-run command validation and manually
          compare the revised skill against run.md.

          ## Acceptance and evidence audit

          `corral criteria list T-227` returned `manifest: null` with spec digest
          `494942b85bd16fb8d96c3f0ca3f85e904e898376090556cb0ac0a883e6dffec5`.
          There are no registered criterion IDs to map. The following maps the actual
          specification requirements instead.

          | Requirement | Current-candidate observation | Assessment |
          |---|---|---|
          | Install through T-226 packaging | Ran `target/debug/corral skill install --dir /tmp/corral-T-227-review.n2JM42`; exit 0. Read the installed worker skill and compared it to the source after accounting for the managed stamp; exact match. `role_skill_markdown` embeds this asset in `crates/corral-ops/src/skill.rs:400`. | Proven. This exercises the installed surface rather than relying on the worker's report. |
          | All five kind variants | Installed skill contains Code change, Spike or research, Docs, Flaky-test fix and Hotfix. Read their complete content. Research requires a report and no production changes; flaky-test guidance calls for deterministic events/barriers/clocks and forbids sleeps, expanded budgets, retries and skips. | Proven by inspecting shipped content. |
          | Every shown command exists; extracting validation check | Ran `scripts/check-docs.sh target/debug/corral` using the candidate's own binary; exit 0, `docs ok: 2056 code spans/blocks, 29 toml examples, 30 files`. The script now includes the three role-skill assets. | Proven for this candidate's documented commands. |
          | Validation can fail | Executed the existing checker's Python functions in memory against `corral run frobnicate`, `corral done --no-such-flag` and `corral nosuchverb`. Asserted exactly one error per probe; all three were rejected; probe runner exit 0. No worktree file was modified. | Proven for invalid top-level verbs, nested verbs and unknown flags. |
          | Contract remains authoritative; no reworded duplicated rules | Compared the complete worker skill directly with the current run.md. Introductory authority claim and step references exist, but duplicated rules and the stricter artifact-count rule remain. | Not met; required change above. The acceptance report's mapping is an assertion, not proof of this semantic requirement. |
          | Lifecycle, AGENTS.md, progress/checkpoint, gate, artifacts, completion | Lifecycle points to contract step 1 for AGENTS.md and worktree boundaries; includes progress/checkpoint readback, payload-guide references, foreground gate procedure, artifact commands, commit/rebase/re-gate, completion preflight and contract step 5 for result/summary shape. | Present, subject to the contract-duplication correction. |
          | Questions, stopping, waits and decisions; harness-neutral replies | Worker lines 47-55 name decision/wait/resume operations and refer to automation.md payloads; reply delivery is described without tying it to a harness-specific tool. Compared with automation.md's explicit waits and decisions section. | Present. |
          | Rework and rebase-only result handling | Rework section covers live request versus retry, review notes, each finding, fresh artifacts/gates and resubmission. It refers to step 5 and also paraphrases its result handling. | Content present; remove the duplicated rule as above. |
          | Cited project lessons and run history | Read `corral lessons --project corral --json`: the four cited lesson IDs exist and support the claims. Read T-045 report row E2 for the 821-SLOC incident and automation.md's stuck-command section. | Supported; citations are grounded in project records. |
          | Out-of-scope findings use submit skill | Final section explicitly routes follow-ups through `corral-submit`, with an agent-origin filing example. | Proven by inspecting shipped content. |
          | Minimal scope and project conventions | Integrated diff contains only the worker asset, skills guide and checker input-glob extension. No production Rust behavior changed and no tests were disabled or weakened. | Met. |

          ## Gate identity and integration

          Inspected the current-attempt receipt
          `/home/pawis/miti-workspace/.corral/artifacts/T-227/R1/f55efc6d-T-227-R1-90411-1792471678334606505.json`.
          Its identity matches the HEAD, tree, integrated base, attempt and spec above.
          It records exit 0 for
          `bash -c 'source scripts/test-herdr.sh && corral_test_herdr_start && scripts/gate.sh'`.
          The associated current gate log at
          `/home/pawis/miti-workspace/.corral/state/gates/T-227-R1-90411-1792471678334606505.log`
          ends with the candidate build, successful docs check, `Gate passed.` and
          `corral gate: pass`. This is machine gate evidence, not the worker's Verified
          paragraph. I did not rerun the full workspace gate.

          Re-reviewed the integrated candidate itself, not only the pre-rebase artifact.
          The integration repair packet names the same current HEAD/base and only the
          three intended changed paths, with no conflict paths or missing base commits.

          The first independent docs-check invocation used a nonexistent umbrella
          `corral/target/debug/corral` path and failed before checking anything. Re-ran
          with this worktree's actual `target/debug/corral`; that run passed as recorded
          above. This was a review-command path error, not a candidate failure.

          No dispatched-worker usability study was performed; shortening time to done
          is unmeasured and is not an explicit acceptance gate for this documentation
          task. The worktree was clean on entry and no review edits were made there.
        sha: 36dc281d9c563f41990424913d2b96a4040b03db
        at: 2026-10-20T05:22:13.059556047Z
        pane: w8V:p4
      usage:
        input: 84230
        output: 6180
        cache_read: 1222016
        cache_write: 0
        cost_usd: 0.3524616
        reported_usd: 0.3524616
        estimated_usd: 0.35246160000000004
        unpriced_tokens: 0
        cost_basis: reported
        entries:
        - at: 2026-10-20T05:22:04.734Z
          latency_ms: 3935
          model: gpt-6.1-sol
          input: 509
          output: 77
          cache_read: 95616
          cache_write: 0
          reported_usd: 0.0113496
          estimated_usd: 0.0113496
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:21:06.216Z
          latency_ms: 15357
          model: gpt-6.1-sol
          input: 957
          output: 660
          cache_read: 91392
          cache_write: 0
          reported_usd: 0.0176532
          estimated_usd: 0.0176532
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:19:26.47Z
          latency_ms: 6857
          model: gpt-6.1-sol
          input: 22758
          output: 125
          cache_read: 15104
          cache_write: 0
          reported_usd: 0.0482764
          estimated_usd: 0.048276400000000004
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:21:17.883Z
          latency_ms: 9860
          model: gpt-6.1-sol
          input: 1181
          output: 376
          cache_read: 92160
          cache_write: 0
          reported_usd: 0.015338
          estimated_usd: 0.015338
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:20:40.373Z
          latency_ms: 7161
          model: gpt-6.1-sol
          input: 3244
          output: 255
          cache_read: 85248
          cache_write: 0
          reported_usd: 0.017562800000000003
          estimated_usd: 0.017562800000000003
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:20:50.666Z
          latency_ms: 9741
          model: gpt-6.1-sol
          input: 3304
          output: 392
          cache_read: 88320
          cache_write: 0
          reported_usd: 0.01936
          estimated_usd: 0.01936
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:22:00.777Z
          latency_ms: 4699
          model: gpt-6.1-sol
          input: 2001
          output: 108
          cache_read: 93824
          cache_write: 0
          reported_usd: 0.0144644
          estimated_usd: 0.014464399999999999
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:21:55.952Z
          latency_ms: 38043
          model: gpt-6.1-sol
          input: 777
          output: 1827
          cache_read: 93184
          cache_write: 0
          reported_usd: 0.0291424
          estimated_usd: 0.029142400000000002
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:19:48.366Z
          latency_ms: 6088
          model: gpt-6.1-sol
          input: 3496
          output: 225
          cache_read: 48896
          cache_write: 0
          reported_usd: 0.0141316
          estimated_usd: 0.014131600000000001
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:19:42.205Z
          latency_ms: 4534
          model: gpt-6.1-sol
          input: 10978
          output: 121
          cache_read: 38144
          cache_write: 0
          reported_usd: 0.0269804
          estimated_usd: 0.0269804
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:20:24.8Z
          latency_ms: 3045
          model: gpt-6.1-sol
          input: 7254
          output: 50
          cache_read: 77952
          cache_write: 0
          reported_usd: 0.022803200000000003
          estimated_usd: 0.0228032
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:19:36.435Z
          latency_ms: 9917
          model: gpt-6.1-sol
          input: 684
          output: 305
          cache_read: 37632
          cache_write: 0
          reported_usd: 0.0081812
          estimated_usd: 0.008181200000000001
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:20:11.644Z
          latency_ms: 15682
          model: gpt-6.1-sol
          input: 14537
          output: 681
          cache_read: 58496
          cache_write: 0
          reported_usd: 0.0417336
          estimated_usd: 0.041733599999999996
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:20:21.513Z
          latency_ms: 9317
          model: gpt-6.1-sol
          input: 5263
          output: 325
          cache_read: 72832
          cache_write: 0
          reported_usd: 0.0210592
          estimated_usd: 0.0210592
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:20:32.837Z
          latency_ms: 8006
          model: gpt-6.1-sol
          input: 493
          output: 270
          cache_read: 84992
          cache_write: 0
          reported_usd: 0.0121852
          estimated_usd: 0.0121852
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:22:09.643Z
          latency_ms: 4706
          model: gpt-6.1-sol
          input: 347
          output: 131
          cache_read: 96000
          cache_write: 0
          reported_usd: 0.011604
          estimated_usd: 0.011604
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T05:19:55.528Z
          latency_ms: 6616
          model: gpt-6.1-sol
          input: 6447
          output: 252
          cache_read: 52224
          cache_write: 0
          reported_usd: 0.0206364
          estimated_usd: 0.020636400000000003
          price_source: config
          unpriced_tokens: 0
    - delivery: legacy
      inbox:
        next_seq: 1
        next_envelope: 1
        open_envelope: null
        effects_applied: []
        binding: null
        observe_cursor:
          offset: 0
          last_complete_line: 0
          coverage: complete
      completion: {}
      id: R1-review2
      agent: gpt-sol
      harness: omo
      model: chatgpt-subscription/gpt-6.1-sol
      effort: high
      vendor: openai
      mode: interactive
      worktree: /home/pawis/miti-workspace/corral/.worktrees/T-227
      branch: corral/T-227
      base: main
      base_sha: 438fb8db83d1cff947dc7251840a5d53bdc0aa82
      herdr:
        workspace_id: w8V
        pane_id: w8V:p6
        socket: /home/pawis/.config/herdr/herdr.sock
      started: 2026-10-20T06:59:01.881270047Z
      ended: 2026-10-20T07:04:29.561351776Z
      seen_working: false
      review:
        identity:
          version: 2
          task: T-227
          run: R1
          attempt: 1731776-1792478412711182080
          head: add53e0d82c529944cde1442a3d193e1ae6437fa
          tree: b98fb9dd5ba76cce96a77dfb0608e88cd37a3fcc
          integration_base: e833a353c1d91d47be2f4f7ad746c88bbac9b17f
          dispatch_base: 438fb8db83d1cff947dc7251840a5d53bdc0aa82
          spec_digest: 494942b85bd16fb8d96c3f0ca3f85e904e898376090556cb0ac0a883e6dffec5
          policy_digest: 7fbc891278f2beaeecdd726ee625e4d17c010fe781736909ce65936cf4094725
          inputs_digest: 1e4a6c744fa6ac2c35b7a6c932be43428163ac0a4e8738eae05db50b6057af9b
          environment:
          - - CARGO_HOME
            - null
          - - CARGO_TARGET_DIR
            - null
          - - CORRAL_HERDR_SESSION
            - null
          - - CORRAL_SIZE_LIMITS
            - '{"rs":800}'
          - - PATH
            - /home/pawis/.local/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.grok/bin:/home/pawis/.bun/bin:/home/pawis/.deno/bin:/home/pawis/.local/bin:/home/pawis/.opencode/bin:/home/pawis/.nvm/versions/node/v24.13.1/bin:/home/pawis/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games:/usr/lib/wsl/lib:/mnt/c/Program Files/PowerShell/7:/mnt/c/Program Files (x86)/Razer Chroma SDK/bin:/mnt/c/Program Files/Razer Chroma SDK/bin:/mnt/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.1/bin/x64:/mnt/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.1/bin:/mnt/c/Windows/system32:/mnt/c/Windows:/mnt/c/Windows/System32/Wbem:/mnt/c/Windows/System32/WindowsPowerShell/v1.0/:/mnt/c/Windows/System32/OpenSSH/:/mnt/c/Program Files/NVIDIA Corporation/NVIDIA App/NvDLISR:/mnt/c/Program Files (x86)/Windows Kits/10/Windows Performance Toolkit/:/mnt/c/Program Files (x86)/NVIDIA Corporation/PhysX/Common:/mnt/c/Program Files/NVIDIA Corporation/Nsight Compute 2025.4.1/:/mnt/c/Program Files/GitHub CLI/:/mnt/c/Program Files/dotnet/:/mnt/c/Program Files/nodejs/:/mnt/c/Windows/system32/config/systemprofile/AppData/Local/Microsoft/WindowsApps:/mnt/c/Windows/system32/config/systemprofile/.dotnet/tools:/mnt/c/Program Files (x86)/Razer/ChromaBroadcast/bin:/mnt/c/Program Files/Razer/ChromaBroadcast/bin:/mnt/c/Users/pawis/AppData/Local/Programs/OpenAI/Codex/bin:/mnt/c/Users/pawis/AppData/Local/hermes/hermes-agent/venv/Scripts:/mnt/c/Users/pawis/AppData/Local/hermes/bin:/mnt/c/Users/pawis/scoop/persist/bun/bin:/mnt/c/Users/pawis/scoop/apps/git/current/cmd:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/site/bin:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/bin:/mnt/c/Users/pawis/scoop/apps/mingw/current/bin:/mnt/c/Users/pawis/scoop/apps/python/current/Scripts:/mnt/c/Users/pawis/scoop/apps/python/current:/mnt/c/Users/pawis/.local/bin:/mnt/c/Users/pawis/.cargo/bin:/mnt/c/Users/pawis/scoop/shims:/mnt/c/Users/pawis/AppData/Local/Microsoft/WindowsApps:/mnt/c/Users/pawis/AppData/Local/Programs/Microsoft VS Code/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Podman/:/mnt/c/Users/pawis/AppData/Local/Programs/Antigravity/bin:/mnt/c/Users/pawis/AppData/Local/Programs/MiKTeX/miktex/bin/x64/:/mnt/c/Users/pawis/.lmstudio/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Ollama:/mnt/c/Users/pawis/AppData/Roaming/npm:/mnt/c/Users/pawis/.dotnet/tools:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe/ripgrep-15.1.0-x86_64-pc-windows-msvc:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin:/mnt/c/Program Files/PowerShell/7/:/mnt/c/Program Files/Git/cmd:/mnt/c/Users/pawis/AppData/Local/hermes/hermes-agent/bin:/mnt/c/Users/pawis/.grok/bin:/mnt/c/Users/pawis/AppData/Local/Programs/OpenAI/Codex/bin:/mnt/c/Users/pawis/AppData/Local/hermes/bin:/mnt/c/Users/pawis/scoop/persist/bun/bin:/mnt/c/Users/pawis/scoop/apps/git/current/cmd:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/site/bin:/mnt/c/Users/pawis/scoop/apps/perl/current/perl/bin:/mnt/c/Users/pawis/scoop/apps/mingw/current/bin:/mnt/c/Users/pawis/scoop/apps/python/current/Scripts:/mnt/c/Users/pawis/scoop/apps/python/current:/mnt/c/Users/pawis/.local/bin:/mnt/c/Users/pawis/.cargo/bin:/mnt/c/Users/pawis/scoop/shims:/mnt/c/Users/pawis/AppData/Local/Microsoft/WindowsApps:/mnt/c/Users/pawis/AppData/Local/Programs/Microsoft VS Code/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Podman/:/mnt/c/Users/pawis/AppData/Local/Programs/Antigravity/bin:/mnt/c/Users/pawis/AppData/Local/Programs/MiKTeX/miktex/bin/x64/:/mnt/c/Users/pawis/.lmstudio/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Ollama:/mnt/c/Users/pawis/AppData/Roaming/npm:/mnt/c/Users/pawis/.dotnet/tools:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe/ripgrep-15.1.0-x86_64-pc-windows-msvc:/mnt/c/Users/pawis/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin:/mnt/c/Users/pawis/AppData/Local/Programs/Cua/cua-driver/bin:/snap/bin:/usr/local/bin:/home/pawis/.local/bin
          - - RUSTC
            - null
          - - RUSTC_WRAPPER
            - null
          - - RUSTFLAGS
            - null
          - - RUSTUP_HOME
            - null
          - - RUSTUP_TOOLCHAIN
            - null
        evidence_digest: 4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945
        verdict: approve
        reviewer: gpt-sol
        notes: |
          # T-227 R1 independent re-review

          Verdict: approve. No blocking findings.

          Reviewed attempt `1731776-1792478412711182080`, HEAD
          `add53e0d82c529944cde1442a3d193e1ae6437fa`, tree
          `b98fb9dd5ba76cce96a77dfb0608e88cd37a3fcc`, integrated base
          `e833a353c1d91d47be2f4f7ad746c88bbac9b17f`.

          ## Findings and previous required change

          The prior P2 finding is resolved. Compared the complete revised skill with
          `crates/corral-ops/assets/run.md`, the Portable resume guidance in
          `docs/guide/run-progress.md`, and the prior reviewer notes
          `artifacts/T-227/R1/a356d46a-review-notes.md`.

          The lifecycle now points to contract steps 1-6 rather than rewording their
          rules. It retains supplementary mechanics: progress payload readback,
          pipefail logging, artifact listing and retention override, the rebase command,
          separate post-rebase evidence, conflict handling, completion preflight,
          harness-neutral replies, durable decisions/waits, and locating review notes.
          The one-artifact-per-criterion requirement is gone. Rework delegates
          rebase-only result preservation to contract step 5. The kind-specific guidance
          and cited lessons supplement rather than redefine the contract.

          ## Acceptance and evidence mapping

          `corral criteria list T-227` returned `manifest: null` and spec digest
          `494942b85bd16fb8d96c3f0ca3f85e904e898376090556cb0ac0a883e6dffec5`.
          There are no registered criterion IDs or observations to map. The following
          maps every specification obligation to this integrated candidate instead.
          Worker reports, artifact titles, and Verified paragraphs were not treated as
          machine PASS evidence.

          | Requirement | Current-candidate evidence and observation | Assessment |
          |---|---|---|
          | Installed through T-226 packaging | Ran `target/debug/corral skill install --dir /tmp/corral-T-227-review-1731776`; exit 0, all four roles installed. Read the installed worker asset and compared it with the source after substituting the managed stamp for `{{marker}}`; exact equality. `crates/corral-ops/src/skill.rs:400` embeds this asset. | Proven on the installed surface. |
          | Every kind variant present | Read the entire source and checked the installed asset for Code change, Spike or research, Docs, Flaky-test fix, and Hotfix headings. Research specifies a report and no production changes. Flaky-test guidance requires deterministic events/barriers/clocks and forbids sleeps, expanded budgets, retries, and skipped tests. | Proven by shipped content inspection, not merely headings. |
          | Extract and validate shown commands | Independently ran `scripts/check-docs.sh target/debug/corral`; exit 0, `docs ok: 2048 code spans/blocks, 29 toml examples, 30 files`. The changed input glob includes all three role-skill assets. Read the checker and the documented command forms. | Proven for the commands shown in this candidate. |
          | Validation can fail | Executed the checker's actual Python functions in memory, without editing files, against `corral nosuchverb`, `corral run frobnicate`, and `corral done --no-such-flag`. Each produced exactly one expected rejection; all assertions passed. | Proven for invalid top-level verbs, nested verbs, and unknown flags. |
          | Contract authority and no differently worded duplicated rules | Manually compared every paragraph with current run.md. The skill uses numbered contract-step references for AGENTS.md, foreground gate, artifact classification/collection, commit/rebase/re-gate, result/summary shape, asking/stopping, and rebase-only result handling. The formerly stricter artifact-count rule is absent. | Met; semantic inspection, not an automated prose check. |
          | Lifecycle, progress, checkpoint, gate, artifacts, commit, rebase, done | Worker lines 17-39 cover the lifecycle through the contract pointers and supplementary commands. Step 1 delegates AGENTS.md to contract step 1; step 7 delegates the result and Verified/Not verified summary shapes to contract step 5. Artifact kinds and tiers remain owned by contract step 3, with listing/set-tier mechanics in the skill. | Complete without duplicating the contract. |
          | Questions, stopping, waits/decisions, harness-neutral delivery | Worker lines 40-47 reference contract step 6, describe delivery without harness-specific tools, and show decision, wait, and resume operations. Compared with `docs/guide/automation.md:571-642`. | Met. |
          | Rework after changes and rebase-only resubmission | Worker lines 82-94 explain live requests versus fresh retries, reading all notes, tracking and addressing findings, reviewing inherited work, and resubmitting through contract steps 2-5. The result decision points to step 5's owning rule. | Met. |
          | Cited failure modes from project lessons/history | Read the four cited lesson entries in `docs/records/T-225-lessons-proposal.json`; their evidence supports the green-gate, foreign-session variant, deterministic race-test, and conflict-invariant claims. Read T-045 report row E2 for T-026's 821-SLOC rejection, and automation.md's watchdog section. | Grounded in identifiable project records; each item has a citation. |
          | Out-of-scope follow-ups through submit skill | Worker lines 119-124 explicitly use `corral-submit`, show an agent-origin filing example, and prohibit expanding the diff for unrelated findings. | Met. |
          | Scope, conventions, tests | The full integrated diff changes only the worker asset, skills guide, and docs-check input glob. No Rust behavior, dependency, or test is changed; no test is weakened or suppressed. `git diff main...HEAD --check` exits 0. | Minimal and in scope. |

          ## Current-attempt gate and integration evidence

          Read and parsed
          `/home/pawis/miti-workspace/.corral/artifacts/T-227/R1/941e2a90-T-227-R1-1731776-1792478412711182080.json`.
          Its attempt, HEAD, tree, base, and spec digest match this review. It records
          `result: pass`, `exit_code: 0`, and exit 0 for:

          ```sh
          bash -c 'source scripts/test-herdr.sh && corral_test_herdr_start && scripts/gate.sh'
          ```

          The matching machine log
          `/home/pawis/miti-workspace/.corral/state/gates/T-227-R1-1731776-1792478412711182080.log`
          ends with the docs checker PASS at line 3505, `Gate passed.` at line 3507,
          and `corral gate: pass` at line 3509. It records two ignored tests, not new
          skips introduced by this documentation-only diff. I did not rerun the full
          workspace gate; I independently reran the affected docs checker and exercised
          installation and negative command validation.

          Read the current integration repair packet
          `artifacts/T-227/R1/f8dcacac-022232093e94e598817e040610372aea4b7a87621ad0f15f23f650eb2561544f.json`.
          It names the same HEAD/base, no missing base commits or conflicts, and exactly
          the three intended paths. Independently re-read the integrated candidate,
          not only a pre-rebase artifact. Final Git readback confirmed HEAD, main, and
          HEAD tree match the identities above.

          ## Limits and worktree preservation

          No dispatched-worker usability trial was performed; reduced time to completion
          is unmeasured, not a required acceptance gate for this content task. Contract
          non-duplication is a manual semantic assessment. The checker is not a general
          shell parser; the actual documented forms passed its checks and inspection.
          No unresolved semantic obligation or missing required live evidence remains.

          The candidate worktree was clean on entry and clean after verification.
          All review notes are outside it; installation used the temporary directory
          above, and negative probes ran in memory. No candidate file was edited.
        sha: add53e0d82c529944cde1442a3d193e1ae6437fa
        at: 2026-10-20T07:04:29.561351776Z
        pane: w8V:p6
      usage:
        input: 98556
        output: 6241
        cache_read: 1036928
        cache_write: 0
        cost_usd: 0.3632148000000001
        reported_usd: 0.3632148000000001
        estimated_usd: 0.3632148000000001
        unpriced_tokens: 0
        cost_basis: reported
        entries:
        - at: 2026-10-20T07:01:17.217Z
          latency_ms: 11879
          model: gpt-6.1-sol
          input: 1824
          output: 174
          cache_read: 49536
          cache_write: 0
          reported_usd: 0.0103416
          estimated_usd: 0.010341600000000001
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:03:01.857Z
          latency_ms: 12608
          model: gpt-6.1-sol
          input: 476
          output: 356
          cache_read: 94208
          cache_write: 0
          reported_usd: 0.0139328
          estimated_usd: 0.0139328
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:04:22.071Z
          latency_ms: 10027
          model: gpt-6.1-sol
          input: 2202
          output: 210
          cache_read: 97920
          cache_write: 0
          reported_usd: 0.016296
          estimated_usd: 0.016296
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:01:40.515Z
          latency_ms: 11350
          model: gpt-6.1-sol
          input: 8320
          output: 295
          cache_read: 61312
          cache_write: 0
          reported_usd: 0.0257212
          estimated_usd: 0.0257212
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:01:27.474Z
          latency_ms: 10068
          model: gpt-6.1-sol
          input: 10333
          output: 206
          cache_read: 51200
          cache_write: 0
          reported_usd: 0.027846
          estimated_usd: 0.027846
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:02:48.615Z
          latency_ms: 13473
          model: gpt-6.1-sol
          input: 5381
          output: 323
          cache_read: 88960
          cache_write: 0
          reported_usd: 0.022888
          estimated_usd: 0.022888
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:02:34.518Z
          latency_ms: 23297
          model: gpt-6.1-sol
          input: 9149
          output: 677
          cache_read: 80000
          cache_write: 0
          reported_usd: 0.033068
          estimated_usd: 0.033068
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:01:04.972Z
          latency_ms: 9645
          model: gpt-6.1-sol
          input: 10951
          output: 192
          cache_read: 38784
          cache_write: 0
          reported_usd: 0.0277004
          estimated_usd: 0.0277004
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:00:54.993Z
          latency_ms: 13677
          model: gpt-6.1-sol
          input: 637
          output: 307
          cache_read: 38272
          cache_write: 0
          reported_usd: 0.0081712
          estimated_usd: 0.0081712
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:00:41.26Z
          latency_ms: 11494
          model: gpt-6.1-sol
          input: 34454
          output: 135
          cache_read: 3968
          cache_write: 0
          reported_usd: 0.0706548
          estimated_usd: 0.0706548
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:03:15.567Z
          latency_ms: 11038
          model: gpt-6.1-sol
          input: 649
          output: 280
          cache_read: 94464
          cache_write: 0
          reported_usd: 0.013544400000000002
          estimated_usd: 0.0135444
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:04:11.877Z
          latency_ms: 45741
          model: gpt-6.1-sol
          input: 1304
          output: 2013
          cache_read: 96768
          cache_write: 0
          reported_usd: 0.0324148
          estimated_usd: 0.0324148
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:03:25.69Z
          latency_ms: 9886
          model: gpt-6.1-sol
          input: 2005
          output: 301
          cache_read: 94976
          cache_write: 0
          reported_usd: 0.0165176
          estimated_usd: 0.016517599999999997
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:01:54.987Z
          latency_ms: 13951
          model: gpt-6.1-sol
          input: 7771
          output: 383
          cache_read: 69504
          cache_write: 0
          reported_usd: 0.026322400000000003
          estimated_usd: 0.026322400000000003
          price_source: config
          unpriced_tokens: 0
        - at: 2026-10-20T07:02:08.165Z
          latency_ms: 12699
          model: gpt-6.1-sol
          input: 3100
          output: 389
          cache_read: 77056
          cache_write: 0
          reported_usd: 0.0177956
          estimated_usd: 0.017795599999999998
          price_source: config
          unpriced_tokens: 0
    usage:
      input: 167297
      output: 40325
      cache_read: 5110454
      cache_write: 231735
      cost_usd: 3.1047945999999977
      reported_usd: 3.1047945999999977
      estimated_usd: 3.104794599999998
      unpriced_tokens: 0
      cost_basis: reported
      entries:
      - at: 2026-10-20T04:08:39.141Z
        latency_ms: 4
        model: claude-opus-5-5
        input: 2
        output: 159
        cache_read: 124745
        cache_write: 860
        reported_usd: 0.032437
        estimated_usd: 0.032437
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:08:26.976Z
        latency_ms: 9
        model: claude-opus-5-5
        input: 2
        output: 1041
        cache_read: 120719
        cache_write: 1730
        reported_usd: 0.0536218
        estimated_usd: 0.053621800000000004
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:40:32.051Z
        latency_ms: 6
        model: claude-opus-5-5
        input: 2
        output: 294
        cache_read: 20477
        cache_write: 107849
        reported_usd: 0.5492284
        estimated_usd: 0.5492284000000001
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:00:28.425Z
        latency_ms: 5
        model: claude-opus-5-5
        input: 2
        output: 571
        cache_read: 113205
        cache_write: 2025
        reported_usd: 0.044194
        estimated_usd: 0.044194
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:40:57.511Z
        latency_ms: 8
        model: claude-opus-5-5
        input: 2
        output: 1966
        cache_read: 128326
        cache_write: 4249
        reported_usd: 0.0862382
        estimated_usd: 0.0862382
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:07:54.696Z
        latency_ms: 12
        model: claude-opus-5-5
        input: 4
        output: 853
        cache_read: 118810
        cache_write: 604
        reported_usd: 0.043858
        estimated_usd: 0.043858
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:08:32.405Z
        latency_ms: 8
        model: claude-opus-5-5
        input: 2
        output: 299
        cache_read: 122449
        cache_write: 1749
        reported_usd: 0.0392228
        estimated_usd: 0.0392228
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:57:36.609Z
        latency_ms: 3
        model: claude-opus-5-5
        input: 2
        output: 1243
        cache_read: 73043
        cache_write: 8206
        reported_usd: 0.08050660000000001
        estimated_usd: 0.08050660000000001
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:49:01.827Z
        latency_ms: 5
        model: claude-opus-5-5
        input: 4
        output: 2309
        cache_read: 145334
        cache_write: 224
        reported_usd: 0.07638280000000001
        estimated_usd: 0.0763828
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:58:16.359Z
        latency_ms: 331
        model: claude-opus-5-5
        input: 2
        output: 207
        cache_read: 95492
        cache_write: 6307
        reported_usd: 0.0547814
        estimated_usd: 0.0547814
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:42:49.25Z
        latency_ms: 4
        model: claude-opus-5-5
        input: 2
        output: 21
        cache_read: 144632
        cache_write: 702
        reported_usd: 0.0328644
        estimated_usd: 0.0328644
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:00:21.803Z
        latency_ms: 14
        model: claude-opus-5-5
        input: 2
        output: 909
        cache_read: 112133
        cache_write: 1072
        reported_usd: 0.0459746
        estimated_usd: 0.045974600000000004
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:01:21.931Z
        latency_ms: 4
        model: claude-opus-5-5
        input: 2
        output: 65
        cache_read: 116949
        cache_write: 1861
        reported_usd: 0.0340028
        estimated_usd: 0.0340028
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:55:52.369Z
        latency_ms: 5
        model: claude-opus-5-5
        input: 2
        output: 421
        cache_read: 48477
        cache_write: 2461
        reported_usd: 0.0304284
        estimated_usd: 0.0304284
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:41:29.355Z
        latency_ms: 9
        model: claude-opus-5-5
        input: 2
        output: 754
        cache_read: 139903
        cache_write: 1285
        reported_usd: 0.0494936
        estimated_usd: 0.049493600000000006
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:41:18.363Z
        latency_ms: 4
        model: claude-opus-5-5
        input: 2
        output: 2707
        cache_read: 132575
        cache_write: 4341
        reported_usd: 0.102368
        estimated_usd: 0.102368
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:00:32.923Z
        latency_ms: 6
        model: claude-opus-5-5
        input: 2
        output: 402
        cache_read: 115230
        cache_write: 998
        reported_usd: 0.036084
        estimated_usd: 0.036084
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:41:22.206Z
        latency_ms: 5
        model: claude-opus-5-5
        input: 2
        output: 162
        cache_read: 136916
        cache_write: 2987
        reported_usd: 0.0455662
        estimated_usd: 0.045566199999999994
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:57:23.437Z
        latency_ms: 4
        model: claude-opus-5-5
        input: 2
        output: 528
        cache_read: 72171
        cache_write: 872
        reported_usd: 0.0293622
        estimated_usd: 0.0293622
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:08:16.289Z
        latency_ms: 10
        model: claude-opus-5-5
        input: 2
        output: 1441
        cache_read: 119414
        cache_write: 1305
        reported_usd: 0.059235800000000005
        estimated_usd: 0.059235800000000005
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:59:32.737Z
        latency_ms: 5
        model: claude-opus-5-5
        input: 2
        output: 729
        cache_read: 103440
        cache_write: 7256
        reported_usd: 0.07155600000000001
        estimated_usd: 0.071556
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:55:55.344Z
        latency_ms: 5
        model: claude-opus-5-5
        input: 2
        output: 253
        cache_read: 50938
        cache_write: 823
        reported_usd: 0.0193706
        estimated_usd: 0.019370599999999998
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:42:17.561Z
        latency_ms: 5
        model: claude-opus-5-5
        input: 2
        output: 389
        cache_read: 142589
        cache_write: 2043
        reported_usd: 0.04652080000000001
        estimated_usd: 0.0465208
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:59:57.242Z
        latency_ms: 3
        model: claude-opus-5-5
        input: 2
        output: 674
        cache_read: 110696
        cache_write: 1437
        reported_usd: 0.042812199999999995
        estimated_usd: 0.042812199999999995
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:08:36.763Z
        latency_ms: 6
        model: claude-opus-5-5
        input: 2
        output: 80
        cache_read: 124198
        cache_write: 547
        reported_usd: 0.029182600000000003
        estimated_usd: 0.029182600000000003
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:58:10.607Z
        latency_ms: 5607
        model: claude-opus-5-5
        input: 2
        output: 1887
        cache_read: 87266
        cache_write: 8226
        reported_usd: 0.0963312
        estimated_usd: 0.09633119999999999
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:42:13.413Z
        latency_ms: 3
        model: claude-opus-5-5
        input: 2
        output: 1484
        cache_read: 141188
        cache_write: 1401
        reported_usd: 0.0649306
        estimated_usd: 0.0649306
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:55:46.458Z
        latency_ms: 6
        model: claude-opus-5-5
        input: 2
        output: 603
        cache_read: 20477
        cache_write: 28000
        reported_usd: 0.1561634
        estimated_usd: 0.1561634
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:57:16.671Z
        latency_ms: 4
        model: claude-opus-5-5
        input: 4
        output: 638
        cache_read: 63403
        cache_write: 8768
        reported_usd: 0.0692966
        estimated_usd: 0.0692966
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:57:41.658Z
        latency_ms: 5
        model: claude-opus-5-5
        input: 2
        output: 479
        cache_read: 81249
        cache_write: 6017
        reported_usd: 0.0559228
        estimated_usd: 0.0559228
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:01:18.621Z
        latency_ms: 16
        model: claude-opus-5-5
        input: 2
        output: 1597
        cache_read: 116228
        cache_write: 721
        reported_usd: 0.058798600000000006
        estimated_usd: 0.058798600000000006
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T04:08:48.977Z
        latency_ms: 4
        model: claude-opus-5-5
        input: 2
        output: 923
        cache_read: 125605
        cache_write: 1526
        reported_usd: 0.051219
        estimated_usd: 0.051219
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:56:06.317Z
        latency_ms: 5
        model: claude-opus-5-5
        input: 2
        output: 741
        cache_read: 51761
        cache_write: 10584
        reported_usd: 0.07810020000000001
        estimated_usd: 0.0781002
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:56:39.662Z
        latency_ms: 4
        model: claude-opus-5-5
        input: 2
        output: 52
        cache_read: 62345
        cache_write: 1058
        reported_usd: 0.018807
        estimated_usd: 0.018807
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T03:59:24.944Z
        latency_ms: 8
        model: claude-opus-5-5
        input: 2
        output: 6985
        cache_read: 101799
        cache_write: 1641
        reported_usd: 0.16827280000000003
        estimated_usd: 0.1682728
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:22:04.734Z
        latency_ms: 3935
        model: gpt-6.1-sol
        input: 509
        output: 77
        cache_read: 95616
        cache_write: 0
        reported_usd: 0.0113496
        estimated_usd: 0.0113496
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:21:06.216Z
        latency_ms: 15357
        model: gpt-6.1-sol
        input: 957
        output: 660
        cache_read: 91392
        cache_write: 0
        reported_usd: 0.0176532
        estimated_usd: 0.0176532
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:19:26.47Z
        latency_ms: 6857
        model: gpt-6.1-sol
        input: 22758
        output: 125
        cache_read: 15104
        cache_write: 0
        reported_usd: 0.0482764
        estimated_usd: 0.048276400000000004
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:21:17.883Z
        latency_ms: 9860
        model: gpt-6.1-sol
        input: 1181
        output: 376
        cache_read: 92160
        cache_write: 0
        reported_usd: 0.015338
        estimated_usd: 0.015338
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:24:30.261Z
        latency_ms: 2934
        model: gpt-6.1-sol
        input: 81858
        output: 27
        cache_read: 15104
        cache_write: 0
        reported_usd: 0.1654964
        estimated_usd: 0.1654964
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:20:40.373Z
        latency_ms: 7161
        model: gpt-6.1-sol
        input: 3244
        output: 255
        cache_read: 85248
        cache_write: 0
        reported_usd: 0.017562800000000003
        estimated_usd: 0.017562800000000003
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:20:50.666Z
        latency_ms: 9741
        model: gpt-6.1-sol
        input: 3304
        output: 392
        cache_read: 88320
        cache_write: 0
        reported_usd: 0.01936
        estimated_usd: 0.01936
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:22:00.777Z
        latency_ms: 4699
        model: gpt-6.1-sol
        input: 2001
        output: 108
        cache_read: 93824
        cache_write: 0
        reported_usd: 0.0144644
        estimated_usd: 0.014464399999999999
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:21:55.952Z
        latency_ms: 38043
        model: gpt-6.1-sol
        input: 777
        output: 1827
        cache_read: 93184
        cache_write: 0
        reported_usd: 0.0291424
        estimated_usd: 0.029142400000000002
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:19:48.366Z
        latency_ms: 6088
        model: gpt-6.1-sol
        input: 3496
        output: 225
        cache_read: 48896
        cache_write: 0
        reported_usd: 0.0141316
        estimated_usd: 0.014131600000000001
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:19:42.205Z
        latency_ms: 4534
        model: gpt-6.1-sol
        input: 10978
        output: 121
        cache_read: 38144
        cache_write: 0
        reported_usd: 0.0269804
        estimated_usd: 0.0269804
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:20:24.8Z
        latency_ms: 3045
        model: gpt-6.1-sol
        input: 7254
        output: 50
        cache_read: 77952
        cache_write: 0
        reported_usd: 0.022803200000000003
        estimated_usd: 0.0228032
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:19:36.435Z
        latency_ms: 9917
        model: gpt-6.1-sol
        input: 684
        output: 305
        cache_read: 37632
        cache_write: 0
        reported_usd: 0.0081812
        estimated_usd: 0.008181200000000001
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:24:33.971Z
        latency_ms: 3645
        model: gpt-6.1-sol
        input: 445
        output: 79
        cache_read: 96768
        cache_write: 0
        reported_usd: 0.0113568
        estimated_usd: 0.0113568
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:20:11.644Z
        latency_ms: 15682
        model: gpt-6.1-sol
        input: 14537
        output: 681
        cache_read: 58496
        cache_write: 0
        reported_usd: 0.0417336
        estimated_usd: 0.041733599999999996
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:22:13.137Z
        latency_ms: 3138
        model: gpt-6.1-sol
        input: 370
        output: 56
        cache_read: 96128
        cache_write: 0
        reported_usd: 0.0109128
        estimated_usd: 0.0109128
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:22:17.77Z
        latency_ms: 4607
        model: gpt-6.1-sol
        input: 318
        output: 117
        cache_read: 96256
        cache_write: 0
        reported_usd: 0.011431600000000002
        estimated_usd: 0.0114316
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:20:21.513Z
        latency_ms: 9317
        model: gpt-6.1-sol
        input: 5263
        output: 325
        cache_read: 72832
        cache_write: 0
        reported_usd: 0.0210592
        estimated_usd: 0.0210592
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:20:32.837Z
        latency_ms: 8006
        model: gpt-6.1-sol
        input: 493
        output: 270
        cache_read: 84992
        cache_write: 0
        reported_usd: 0.0121852
        estimated_usd: 0.0121852
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:22:09.643Z
        latency_ms: 4706
        model: gpt-6.1-sol
        input: 347
        output: 131
        cache_read: 96000
        cache_write: 0
        reported_usd: 0.011604
        estimated_usd: 0.011604
        price_source: config
        unpriced_tokens: 0
      - at: 2026-10-20T05:19:55.528Z
        latency_ms: 6616
        model: gpt-6.1-sol
        input: 6447
        output: 252
        cache_read: 52224
        cache_write: 0
        reported_usd: 0.0206364
        estimated_usd: 0.020636400000000003
        price_source: config
        unpriced_tokens: 0
    autopilot:
      stage: reviewed
      sha: add53e0d82c529944cde1442a3d193e1ae6437fa
      at: 2026-10-20T07:03:20.295913912Z
  artifacts:
  - run: R1
    kind: log
    title: Full gate on rebased tree (main 438fb8d)
    path: artifacts/T-227/R1/9f812136-gate.log
    source: /home/pawis/miti-workspace/corral/.worktrees/T-227/.omo/evidence/gate.log
    sha256: 9f8121369ac2fde5b1862aa2a9b6f9f42bb5065468467edaf13677cc87613a68
    bytes: 263149
    added: 2026-10-20T04:07:55.766839404Z
  - run: R1
    kind: report
    title: 'Acceptance report: install, variants, verb check, run.md mapping'
    path: artifacts/T-227/R1/5868bf57-T-227-skill-report.md
    source: /home/pawis/miti-workspace/corral/.worktrees/T-227/.omo/evidence/T-227-skill-report.md
    sha256: 5868bf5729e16601671779cca1be5d2a72c82b6aca93a38da2c2ffc60eb8d2b7
    bytes: 3366
    added: 2026-10-20T04:07:57.535464255Z
  - tier_override: keep
    run: R1
    kind: evidence
    title: Verb check fails on injected unknown verbs and flag
    path: artifacts/T-227/R1/ec025235-verb-check-negative.log
    source: /home/pawis/miti-workspace/corral/.worktrees/T-227/.omo/evidence/verb-check-negative.log
    sha256: ec025235e00fb7cf8b977fa951e8000e77aaf0db575327689b96e308354a8755
    bytes: 273
    added: 2026-10-20T04:07:59.132030444Z
  - run: R1
    kind: log
    title: Verb check passes on README, guides and role skills
    path: artifacts/T-227/R1/c88696a2-verb-check-pass.log
    source: /home/pawis/miti-workspace/corral/.worktrees/T-227/.omo/evidence/verb-check-pass.log
    sha256: c88696a262b50e02fee1b0dbd4aa979975aeaedcd0f9698825a4a801bdb62e37
    bytes: 60
    added: 2026-10-20T04:08:00.11626707Z
  - run: R1
    kind: evidence
    title: corral skill install writes worker skill with all variants
    path: artifacts/T-227/R1/60fbedf9-skill-install.log
    source: /home/pawis/miti-workspace/corral/.worktrees/T-227/.omo/evidence/skill-install.log
    sha256: 60fbedf914ae7a9c4e990ea2bca8f93faef37513339016a11c8ff86d801f39c7
    bytes: 1241
    added: 2026-10-20T04:08:01.04156458Z
  - run: R1
    kind: evidence
    title: Resume checkpoint 1
    path: artifacts/T-227/R1/d4e28926fb388f598223957abfe5707e0363efddbbc8cfb462b209d7cf52044f-checkpoint-1.json
    source: /home/pawis/miti-workspace/.corral/artifacts/T-227/R1/d4e28926fb388f598223957abfe5707e0363efddbbc8cfb462b209d7cf52044f-checkpoint-1.json
    sha256: d4e28926fb388f598223957abfe5707e0363efddbbc8cfb462b209d7cf52044f
    bytes: 2699
    added: 2026-10-20T04:08:28.754043617Z
  - tier_override: transient
    run: R1
    kind: diff
    title: Final diff +131 −14
    path: artifacts/T-227/R1/9c9f973d-final.diff
    source: /home/pawis/miti-workspace/.corral/state/tmp/T-227-R1/final.diff
    sha256: 9c9f973d6d0630f7a0bea3d08482a38fc16cbb0bb1dcea1fa41d996c53695407
    bytes: 12023
    added: 2026-10-20T04:08:34.221225426Z
  - run: R1
    kind: summary
    title: Done summary
    path: artifacts/T-227/R1/880e5cc1-summary.md
    source: /home/pawis/miti-workspace/.corral/state/tmp/T-227-R1/summary.md
    sha256: 880e5cc181954fc2fda61e890da587db7652a8c0693e1cacf0fa60a55fd3c5d4
    bytes: 2174
    added: 2026-10-20T04:08:34.580727302Z
  - run: R1
    kind: log
    title: 'Gate: pass'
    path: artifacts/T-227/R1/90a9e8fe-T-227-R1-3300182-1792471100273098719.log
    source: /home/pawis/miti-workspace/.corral/state/gates/T-227-R1-3300182-1792471100273098719.log
    sha256: 90a9e8fe22d4c34182e3ce8bc4844994f318f1d94d70f0888fde6ce3872b7b9d
    bytes: 245457
    added: 2026-10-20T04:46:02.762172099Z
  - run: R1
    kind: evidence
    title: Gate command receipts
    path: artifacts/T-227/R1/1c5d29e6-T-227-R1-3300182-1792471100273098719.json
    source: /home/pawis/miti-workspace/.corral/state/gates/T-227-R1-3300182-1792471100273098719.json
    sha256: 1c5d29e6f48cca7b8514b3dfbd6c05db86ebd6917c07a1521debe123414e3d6a
    bytes: 5615
    added: 2026-10-20T04:46:07.292805297Z
  - run: R1
    kind: report
    title: Integration repair packet
    path: artifacts/T-227/R1/c8ea8b7a-59defbc4e5f4454e8ee910aabc5941fe88a8070d2ab863ba3f03d9d0287a40dc.json
    source: /home/pawis/miti-workspace/.corral/state/integration/59defbc4e5f4454e8ee910aabc5941fe88a8070d2ab863ba3f03d9d0287a40dc.json
    sha256: c8ea8b7a78eb30742c73a99441ca714bee83f099e2186eb0bbd4c896277722b9
    bytes: 2350
    added: 2026-10-20T04:46:11.709428866Z
  - run: R1
    kind: log
    title: 'Gate: pass'
    path: artifacts/T-227/R1/d365c5c9-T-227-R1-90411-1792471678334606505.log
    source: /home/pawis/miti-workspace/.corral/state/gates/T-227-R1-90411-1792471678334606505.log
    sha256: d365c5c97eb03a3e2106b2b05042e3e907409142c3276b3c4b17b98d96eca573
    bytes: 247501
    added: 2026-10-20T04:59:27.786425021Z
  - run: R1
    kind: evidence
    title: Gate command receipts
    path: artifacts/T-227/R1/f55efc6d-T-227-R1-90411-1792471678334606505.json
    source: /home/pawis/miti-workspace/.corral/state/gates/T-227-R1-90411-1792471678334606505.json
    sha256: f55efc6dec28b8a467d70acda60e32922cb0462566f4418ecbbc4a52677995c1
    bytes: 5586
    added: 2026-10-20T04:59:34.380065575Z
  - run: R1
    kind: report
    title: Review (gpt-sol)
    path: artifacts/T-227/R1/a356d46a-review-notes.md
    source: /home/pawis/miti-workspace/.corral/state/tmp/T-227-R1-review/review-notes.md
    sha256: a356d46a42b3c7e254a9c567c5409117f4abec12b69b31a230663d0fc32134c3
    bytes: 7111
    added: 2026-10-20T05:22:52.187003599Z
  - run: R1
    kind: log
    title: Reviewer transcript (R1-review1)
    path: artifacts/T-227/R1/fe907c9f-R1-review1.transcript.log
    source: /home/pawis/miti-workspace/.corral/state/review-watch/T-227/R1-review1.transcript.log
    sha256: fe907c9fa633b59cc7f2d434bcba5218ce55e759959681550408aac3ed625823
    bytes: 530153
    added: 2026-10-20T05:24:20.031096495Z
  - run: R1
    kind: report
    title: Autopilot changes (round 1)
    path: artifacts/T-227/R1/8e80ecbc-changes-round-1.md
    source: /home/pawis/miti-workspace/.corral/state/tmp/T-227-R1-autopilot/changes-round-1.md
    sha256: 8e80ecbcff3a97693fb760653a0337013a2f9d5dae3cd32a8dee4db4b845e5c7
    bytes: 7332
    added: 2026-10-20T05:24:23.044401452Z
  - run: R1
    kind: log
    title: Reviewer screen (R1-review1)
    path: artifacts/T-227/R1/3fad26b3-R1-review1.screen.log
    source: /home/pawis/miti-workspace/.corral/state/review-watch/T-227/R1-review1.screen.log
    sha256: 3fad26b354e8e19404bd9ea3e4fcc79a55fdb3bb503876ce6c2e6f4855b1f4c6
    bytes: 4108
    added: 2026-10-20T05:24:25.909566573Z
  - run: R1
    kind: log
    title: Full gate on reworked tree rebased on main e833a35
    path: artifacts/T-227/R1/4a875dac-gate-r2.log
    source: /home/pawis/miti-workspace/corral/.worktrees/T-227/.omo/evidence/gate-r2.log
    sha256: 4a875dac22f74a838a2b632507f08e5be11ecd68e6aa1339b661dccbd68f8dd9
    bytes: 247272
    added: 2026-10-20T05:49:02.475317532Z
  - run: R1
    kind: report
    title: 'Round 1 rework: run.md duplication removed, comparison'
    path: artifacts/T-227/R1/23c140fb-T-227-round1-rework-report.md
    source: /home/pawis/miti-workspace/corral/.worktrees/T-227/.omo/evidence/T-227-round1-rework-report.md
    sha256: 23c140fbb28eda39e9fc1c01a9299947b1f924e6cd4b85f0fe46d0ec5d7cf5c2
    bytes: 3360
    added: 2026-10-20T05:49:03.493279298Z
  - run: R1
    kind: log
    title: Verb check passes on reworked skill
    path: artifacts/T-227/R1/9b1bcfba-verb-check-pass-r2.log
    source: /home/pawis/miti-workspace/corral/.worktrees/T-227/.omo/evidence/verb-check-pass-r2.log
    sha256: 9b1bcfba486e0fb0defd9881e0a3bd2f992dd238ef80923e8eb6ece8d61b58f2
    bytes: 60
    added: 2026-10-20T05:49:04.527180007Z
  - run: R1
    kind: evidence
    title: Resume checkpoint 2
    path: artifacts/T-227/R1/b2bb56af1a20a6b30c5590c5bdbbf9fe25b04f02dba94cc1049613fcab15fded-checkpoint-2.json
    source: /home/pawis/miti-workspace/.corral/artifacts/T-227/R1/b2bb56af1a20a6b30c5590c5bdbbf9fe25b04f02dba94cc1049613fcab15fded-checkpoint-2.json
    sha256: b2bb56af1a20a6b30c5590c5bdbbf9fe25b04f02dba94cc1049613fcab15fded
    bytes: 3249
    added: 2026-10-20T05:49:08.407704985Z
  - run: R1
    kind: diff
    title: Final diff +121 −14
    path: artifacts/T-227/R1/08a848a6-final.diff
    source: /home/pawis/miti-workspace/.corral/state/tmp/T-227-R1/final.diff
    sha256: 08a848a61b8832504167eb3823252ed3ca80ac02742e1e797df8fcf8a97a77d0
    bytes: 11180
    added: 2026-10-20T05:49:11.898838819Z
  - run: R1
    kind: summary
    title: Done summary
    path: artifacts/T-227/R1/8d564b59-summary.md
    source: /home/pawis/miti-workspace/.corral/state/tmp/T-227-R1/summary.md
    sha256: 8d564b5991251617e69402ad5c64ef42f70bb9c3c775fd672e14f802ad4510d9
    bytes: 1730
    added: 2026-10-20T05:49:12.325481525Z
  - run: R1
    kind: evidence
    title: T-225-lessons-proposal.json
    path: artifacts/T-227/R1/ba2869ab-T-225-lessons-proposal.json
    source: /home/pawis/miti-workspace/corral/.worktrees/T-227/docs/records/T-225-lessons-proposal.json
    sha256: ba2869abd670af701a5e8a92f9892f2186257ae7332358cd1ef76640c2bde6ee
    bytes: 13609
    added: 2026-10-20T05:49:12.774306881Z
  - run: R1
    kind: report
    title: T-225-lessons-report.md
    path: artifacts/T-227/R1/9f74becb-T-225-lessons-report.md
    source: /home/pawis/miti-workspace/corral/.worktrees/T-227/docs/records/T-225-lessons-report.md
    sha256: 9f74becb7cbdebcad73f54698e76fcb2e5fb2336fff1f6750215f554c6511f33
    bytes: 2982
    added: 2026-10-20T05:49:13.185151935Z
  - run: R1
    kind: report
    title: Integration repair packet
    path: artifacts/T-227/R1/f8dcacac-022232093e94e598817e040610372aea4b7a87621ad0f15f23f650eb2561544f.json
    source: /home/pawis/miti-workspace/.corral/state/integration/022232093e94e598817e040610372aea4b7a87621ad0f15f23f650eb2561544f.json
    sha256: f8dcacac3d224fae3621ecff1407f273aa9d3bc5aa04540255481e533c4f8b56
    bytes: 2537
    added: 2026-10-20T05:50:05.027940836Z
  - run: R1
    kind: log
    title: 'Gate: pass'
    path: artifacts/T-227/R1/9cf2906b-T-227-R1-1731776-1792478412711182080.log
    source: /home/pawis/miti-workspace/.corral/state/gates/T-227-R1-1731776-1792478412711182080.log
    sha256: 9cf2906b4b620023787598945856e68d87b784c97dd8a205bc104e348da52926
    bytes: 245773
    added: 2026-10-20T06:47:06.055573875Z
  - run: R1
    kind: evidence
    title: Gate command receipts
    path: artifacts/T-227/R1/941e2a90-T-227-R1-1731776-1792478412711182080.json
    source: /home/pawis/miti-workspace/.corral/state/gates/T-227-R1-1731776-1792478412711182080.json
    sha256: 941e2a9062fcdf46430729417ccaa333ea44ec55d3ab1584f0671a801153ce75
    bytes: 5321
    added: 2026-10-20T06:47:07.102206749Z
  - run: R1
    kind: report
    title: Review (gpt-sol)
    path: artifacts/T-227/R1/6ec079ad-review-notes.md
    source: /home/pawis/miti-workspace/.corral/state/tmp/T-227-R1-review/review-notes.md
    sha256: 6ec079ad37a18b650a449110c9d2eb55868817d27e437e26b737d3c11f6cf35f
    bytes: 7445
    added: 2026-10-20T07:04:44.979130952Z
  - run: R1
    kind: log
    title: Reviewer transcript (R1-review2)
    path: artifacts/T-227/R1/7b19ee22-R1-review2.transcript.log
    source: /home/pawis/miti-workspace/.corral/state/review-watch/T-227/R1-review2.transcript.log
    sha256: 7b19ee22fbc751c107dbc918c9865271ded8b15595319726a44925ce3c9f6906
    bytes: 562184
    added: 2026-10-20T07:05:36.157439681Z
  - run: R1
    kind: report
    title: Integration repair packet
    path: artifacts/T-227/R1/6c7fff1c-022232093e94e598817e040610372aea4b7a87621ad0f15f23f650eb2561544f.json
    source: /home/pawis/miti-workspace/.corral/state/integration/022232093e94e598817e040610372aea4b7a87621ad0f15f23f650eb2561544f.json
    sha256: 6c7fff1c577087aa29a05194715eef148634b8db9ba44664e0adbf4a0514e8bb
    bytes: 10166
    added: 2026-10-20T07:05:38.464734841Z
  - run: R1
    kind: log
    title: Reviewer screen (R1-review2)
    path: artifacts/T-227/R1/38c23249-R1-review2.screen.log
    source: /home/pawis/miti-workspace/.corral/state/review-watch/T-227/R1-review2.screen.log
    sha256: 38c23249013a78e0567d2dd56fe5c5218182444e77d477541cea37971795aad0
    bytes: 4667
    added: 2026-10-20T07:05:39.317942916Z
---
## Goal
Dispatched workers sometimes spend time figuring out how to finish a task (gate, artifacts, rebase, `done` shape, rework). Write the `corral-worker` skill content so the procedure is at hand. `run.md` (T-032) stays the authoritative contract; the skill explains how to carry it out and must not redefine or contradict it.

## Content
- Lifecycle walkthrough: read AGENTS.md, `run progress` / `run checkpoint`, gate in the foreground, artifact kinds and tiers, commit, rebase onto base and re-gate, `done --result/--summary` shape (Verified / Not verified), asking a question and stopping, `run wait` / `run decision`.
- Variants by task kind: code change, spike/research (report artifact, no product code), docs, flaky-test fix (deterministic, no sleeps or loosened budgets), hotfix.
- Rework after a `changes` verdict: read the review notes, address each finding, re-gate, resubmit; rebase-only resubmission and when to keep `--result`.
- Known failure modes, grounded in project lessons (e.g. green-gate-not-acceptance) and run history; each cited.
- Out-of-scope findings: file follow-ups via the submit skill instead of widening scope.

## Acceptance
- Skill installed via the T-226 packaging; each kind variant present; every command shown exists in `corral --help` (test or check script that extracts and validates verbs).
- No rule duplicated from run.md with different wording; the skill links to the contract instead.

## Classification
docs, m: one skill body with kind variants plus a verb-validity check. Related: T-032 (run contract, authoritative), T-154 (review contracts own reviewer guidance; no reviewer skill), T-064 (lessons), T-206..T-210 (agent inbox; describe reply delivery harness-neutrally).

## Filing
Filed by the PM at the owner's request (2026-10-20). Filing is not dispatch authorization; the owner dispatches it. Outside the current autopilot grant scope.

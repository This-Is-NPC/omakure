# Relatório de falhas CI — matriz v0.3 integration

**Data:** 2026-09-04  
**Branch alvo:** `feat/v0.3-integration` → `master`  
**SHAs de referência:**

| Ref | SHA |
|-----|-----|
| `master` (inalterado) | `cb8cdc673a99d2886745b62220d8d2873a734549` |
| PR #39 merge em `feat/v0.3-integration` | `561938d3cff8dbb74a6fa465a368cbbb570e5c84` |
| Run pré-PR39 | `32751e4` (completo no run 33864582115) |
| Run pós-PR39 | `561938d` (completo no run 33868733559) |

**Links:**

| Recurso | URL |
|---------|-----|
| PR #37 (matriz ativa) | https://github.com/This-Is-NPC/omakure/pull/37 |
| PR #39 (fix CI) | https://github.com/This-Is-NPC/omakure/pull/39 |
| Run 33864582115 (pré-PR39, 11 pass / 6 fail) | https://github.com/This-Is-NPC/omakure/actions/runs/33864582115 |
| Run 33868733559 (pós-PR39, 13 pass / 4 fail) | https://github.com/This-Is-NPC/omakure/actions/runs/33868733559 |

> O `ci.yml` dispara em `push`/`pull_request` apenas para `master`. A matriz relevante é a do **PR #37** (head `feat/v0.3-integration`).

---

## Resumo executivo

| Falha | Run inicial | Run atual (pós-PR39) | Status atual | Classe |
|-------|-------------|----------------------|--------------|--------|
| macOS x86_64 + ARM64: `cargo_args[@]` unbound (`set -u` / Bash 3.2) | 33864582115 ❌ | 33868733559 ✅ | **Corrigido** | Bash nounset + array vazio |
| Linux gnu x86_64: `DependencyMissing { name: "python3" }` | 33864582115 ❌ | 33868733559 ✅ (outro teste falhou) | **Corrigido** | PATH re-injetado em spawn absoluto |
| Linux musl x86_64: `git_askpass_refuses_credentials_for_another_host` ETXTBSY | 33864582115 ❌ | 33868733559 ✅ | **Corrigido** | exec com fd aberto / chmod durante write |
| Windows x86_64 + ARM64: `exact.battery-list` git_url vs fixture | 33864582115 ❌ | 33868733559 ❌ (outro probe) | **Parcial** — list corrigido, add ainda falha | Asserção de probe desalinhada |
| Linux gnu x86_64: `git_askpass_refuses_credentials_for_another_host` ETXTBSY | — | 33868733559 ❌ job `101009439140` | **Aberto** | race ETXTBSY residual pós-rename chmod |
| Linux ARM64 gnu: `test_dependency_checks_use_injected_path_for_every_runtime` `DependencyMissing { git }` | — (ARM64 verde na run anterior) | 33868733559 ❌ job `101009439120` | **Aberto** | regressão PATH-skip + env herdado |
| Windows x86_64 + ARM64: `mismatch.battery-add` | — | 33868733559 ❌ jobs `101009439244`, `101009439103` | **Aberto** | probe compara `cli_url` raw vs `normalize_git_url` |

---

## Linha do tempo

1. **PR #37 aberto** — `feat/v0.3-integration` → `master`. Matriz de 9 jobs (Linux gnu/musl × x86_64/aarch64, macOS × 2, Windows × 2).
2. **PR #38 merged** — trabalho Windows incorporado na branch feat.
3. **Run 33864582115** (SHA `32751e4`) — **6 falhas / 11 passos**.
4. **PR #39 merged** (2026-09-04T11:36:08Z, SHA `561938d3cff8dbb74a6fa465a368cbbb570e5c84`) — correções em `build-release`, `system_checks`, `battery.rs`, probe `battery-list`.
5. **Run 33868733559** (SHA `561938d`) — **4 falhas / 13 passos** (~12 min). macOS, musl e python3 corrigidos; restam Windows add, Linux gnu askpass flake, Linux ARM64 PATH.

---

## O que o PR #39 corrigiu (e o que NÃO corrigiu)

### Corrigido ✅

| Área | Mudança | Evidência no código |
|------|---------|---------------------|
| `scripts/tasks/atomic/build-release` | Expansão nounset-safe de `cargo_args` vazio | linhas 29–30: `${cargo_args[@]+"${cargo_args[@]}"}` |
| `src/adapters/system_checks.rs` | Não reaplicar `PATH` quando o binário já é absoluto | linhas 40–47 |
| `src/operations/battery.rs` | `write_secret_file`: drop fd + sync antes de chmod; temp+rename para askpass; dir único por thread-id | linhas 656–657, 595–603, 535–539 |
| `tests/behavioral_parity/battery.rs` | `exact.battery-list` compara via `normalized_local_git_url` | linhas 86–90 |

### NÃO corrigido ❌

| Falha residual | Motivo |
|----------------|--------|
| `mismatch.battery-add` (Windows) | Probe `battery_add_policy_mismatch` ainda compara `git_url == cli_url` raw (linha 259); `add_battery` persiste URL normalizada (linha 214) |
| `git_askpass_refuses_credentials_for_another_host` (Linux gnu) | `set_permissions` pós-rename em `prepare_git_askpass` (linhas 604–621) fora de `write_secret_file`; race ETXTBSY sob paralelismo CI |
| `test_dependency_checks_use_injected_path_for_every_runtime` (Linux ARM64) | PATH-skip deixa child herdar PATH gigante do runner GHA em vez de `PATH=tmpdir` do teste (linhas 40–47); `Err(_)` → `DependencyMissing` oculta errno real (linhas 91–94) |

Os três sintomas abertos quebram **duas invariantes**, não três kernels. O conserto consistente aplica a mesma primitiva em todos os SO:

| Invariante | Falha que a viola | Primativa |
|-----------|-------------------|-----------|
| Identidade persistida = forma que o teste compara | Windows `mismatch.battery-add` | `normalized_local_git_url` em todo probe de path |
| Filho nunca herda PATH acidental; arquivo gerado já nasce no modo final | ARM64 PATH + gnu ETXTBSY | PATH explícito e limitado no spawn; `open(tmp, modo_final)` → close → rename → exec |

---

## Falha 1 — Windows `mismatch.battery-add`

### Sintomas

- Job Windows x86_64 (`101009439244`) e ARM64 (`101009439103`) falham no probe `mismatch.battery-add`.
- Mensagem: `CLI local battery add did not register the requested source`.

### Evidência

O probe verifica registro comparando a URL **literal** passada na CLI:

```256:261:tests/behavioral_parity/battery.rs
    let cli_registered = cli_state["data"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["name"] == BATTERY))
        .is_some_and(|item| item["git_url"] == cli_url);
    if !cli_registered {
        return Err("CLI local battery add did not register the requested source".into());
```

`cli_url` vem de `fixture.repository.to_str()` — no Windows, caminho temporário não canônico (ex.: sem prefixo `\\?\`).

`add_battery` normaliza antes de persistir:

```214:214:src/operations/battery.rs
    let git_url = normalize_git_url(&request.git_url)?;
```

`normalize_git_url` faz `canonicalize()` e remove prefixo verbatim Windows:

```1369:1377:src/operations/battery.rs
    let canonical = path.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("battery local git source must exist: {err}"),
        )
    })?;
    Ok(strip_windows_verbatim_owned(
        canonical.to_string_lossy().into_owned(),
    ))
```

O probe `exact.battery-list` **já** usa `normalized_local_git_url` (linhas 39–42, 86).

### Causa — **PROVEN**

Bug de **asserção no probe**, não de produção. O `add` provavelmente registrou a bateria com URL canônica; o probe exige igualdade com a string raw da CLI. No Windows, `canonicalize` + strip `\\?\` diverge do `to_str()` do tempfile.

### Por que `check:full` local passa

- Host local: Linux x86_64 gnu (`scripts/tasks/check/full` → `scripts/tasks/check/platform/linux-gnu` apenas).
- Em Linux, `tempfile` + `canonicalize` frequentemente produzem string idêntica à passada na CLI.
- Probe de paridade comportamental roda na suíte nativa, mas a divergência só aparece com semântica de path Windows.

### Proposta de correção

Alinha com o padrão Rust para `canonicalize` no Windows ([rust-lang/rust#42869](https://github.com/rust-lang/rust/issues/42869), crate [`dunce`](https://crates.io/crates/dunce)): produção persiste a forma interoperável; teste afirma **a mesma função**. Não gravar a string crua da CLI.

1. Em `battery_add_policy_mismatch`, comparar `git_url` do list com `normalized_local_git_url(&cli_ctx.fixture.repository)` — mesmo padrão de `battery_list`.
2. **Não** alterar `add_battery` só para satisfazer o teste; a normalização em produção é intencional.
3. Rede de segurança local: fixture Linux com symlink (`repo-link` → `repo-real`) para `to_str()` divergir de `canonicalize` sem runner Windows.
4. `dunce` é opcional. O helper interno (`strip_windows_verbatim_owned`) já existe; unificar probes nele. Não fazer strip cego de `\\?\` em todo path (quebra UNC real e nomes reservados como `COM`/`NUL`).

---

## Falha 2 — Linux gnu `git_askpass_refuses_credentials_for_another_host` ETXTBSY

### Sintomas

- Run 33868733559, job Linux headless gnu x86_64 (`101009439140`).
- Teste: `operations::battery::tests::git_askpass_refuses_credentials_for_another_host`.
- Erro: `Os { code: 26, kind: Uncategorized, message: "Text file busy" }` (ETXTBSY) em `Command::new(askpass.sh).output().unwrap()` (linha 4737).

### Evidência

Fluxo `prepare_git_askpass`:

1. `write_secret_file` para token e script temp — fecha fd antes de chmod (linhas 656–657).
2. `fs::rename` temp → `askpass.sh` (linhas 597–603).
3. **`set_permissions(0o700)` no script já renomeado** (linhas 604–621) — fora do guard de `write_secret_file`.
4. Teste executa imediatamente (linhas 4733–4737).

PR #39 corrigiu o caso musl (write+chmod com fd aberto); musl passou na run pós-merge. gnu ainda falha com race residual.

### Causa

- **PROVEN:** `execve` enquanto inode ainda "busy" (ETXTBSY) — kernel Linux rejeita execução quando há operação de metadata pendente ou janela entre rename/chmod e exec.
- **HIPÓTESE:** Paralelismo `cargo test --lib` no runner GHA + overlay FS amplia a janela; stress local 50× passou, CI não.

### Por que musl passou e gnu não

- Mesmo código, targets diferentes; musl tinha falhado na run **anterior** por fd aberto durante chmod (corrigido em `write_secret_file`).
- gnu x86_64 na run pós-PR39 falhou neste teste (não no PATH); sugere flake dependente de timing/FS do runner `ubuntu-latest`, não diferença semântica musl vs gnu.

### Proposta de correção

ETXTBSY no `execve` significa **inode com fd gravável ainda aberto** ([man execve](https://man7.org/linux/man-pages/man2/execve.2.html), [LWN](https://lwn.net/Articles/866493/), [SO: fd do `mkstemp` vazado](https://stackoverflow.com/questions/28639029/execve-to-file-i-just-wrote-text-file-busy)). `chmod` depois do `rename` não é o padrão Unix; em overlayfs (imagem do runner GHA) metadata pode disparar copy-up e reabrir o arquivo para escrita.

A receita de instaladores (rpm/dpkg/cargo) é mode no create, close, rename, exec — **sem** `set_permissions` no path visível:

```text
open(tmp, O_CREAT|O_EXCL, modo_final)   # 0700 no script, 0600 no token
write + fsync
close                                   # único sinal portátil de “não busy”
rename(tmp → nome_final)                # o nome visível já nasce executável
exec
```

No código isso está partido: `write_secret_file` abre com `0o600` e chmod de novo depois de fechar; `prepare_git_askpass` faz rename e chmod `0o700` no `askpass.sh` já visível. O segundo passo é a janela gnu. musl passou no PR #39 porque o bug dominante era fd aberto durante o write.

|Ação|Detalhe|
|---|---|
|Mode no `open` do temp|Script nasce `0700`; token continua `0600`. `write_secret_file` deixa de chmod depois de fechar|
|Sem chmod pós-rename|`prepare_git_askpass` não chama `set_permissions` no path final|
|Mesmo helper para shims|O teste de PATH em `system_checks.rs` (write + `set_mode(0o755)` + exec imediato) usa a mesma primitiva — senão o errno some em `DependencyMissing { git }`|
|Teste sem `unwrap`|Linhas 4733–4747: propagar `io::Error` com contexto|
|`fsync` do diretório|Bônus após `rename` em overlayfs; **não** substitui close-before-exec|
|Retry ETXTBSY|Só cinto de segurança no helper de exec (2–3×, sleep curto). **Não** é a estratégia|
|**Não fazer**|Silenciar o teste; enfraquecer host-bound token; `O_TMPFILE`+`linkat` (Linux-only — quebra macOS/Windows/musl)|

Windows não tem ETXTBSY; o equivalente é sharing violation se o handle ainda estiver aberto. A mesma primitiva (close antes de qualquer uso) cobre os dois. `GIT_ASKPASS` host-bound (`OMAKURE_GIT_AUTHORITY`) permanece o padrão certo para token de curta duração ([gitcredentials(7)](https://git-scm.com/docs/gitcredentials)); não trocar por helper persistente nem `http.extraheader` do `actions/checkout`.

---

## Falha 3 — Linux ARM64 `test_dependency_checks_use_injected_path_for_every_runtime`

### Sintomas

- Run 33868733559, job Linux ARM64 headless gnu (`101009439120`).
- `adapters::system_checks::tests::test_dependency_checks_use_injected_path_for_every_runtime` → `DependencyMissing { name: "git" }`.
- **Mesmo job ARM64 era SUCCESS na run 33864582115** (pré-PATH-skip).

### Evidência

Teste cria shims em `tmpdir` e injeta `PATH=tmpdir`:

```327:342:src/adapters/system_checks.rs
        let dir = tempfile::tempdir().unwrap();
        let programs = ["git", "jq", "bash", python_program(), powershell_program()];
        // ...
        let env = vec![("PATH".to_string(), dir.path().display().to_string())];
```

Fluxo de resolução:

```18:27:src/adapters/system_checks.rs
    let Some(injected_path) = crate::runtime::path_value(env) else {
        return ensure_command(program, args, not_found_hint);
    };
    let Some(path) = crate::runtime::resolve_program_in_path(program, injected_path) else {
        return Err(ScriptError::DependencyMissing { ... });
    };
    ensure_command_os_with_env(path, program, args, not_found_hint, env)
```

Com binário absoluto, PATH é **omitido** no spawn:

```40:47:src/adapters/system_checks.rs
    if std::path::Path::new(program).is_absolute() {
        // Lookup already used the injected PATH; re-applying a huge inherited
        // PATH to the probe spawn can break exec on some Linux runners.
        for (key, value) in env {
            if !key.eq_ignore_ascii_case("PATH") {
                command.env(key, value);
            }
        }
```

Qualquer `output()` Err vira `DependencyMissing` genérico:

```91:94:src/adapters/system_checks.rs
        Err(_) => Err(ScriptError::DependencyMissing {
            name: name.to_string(),
            hint: not_found_hint.to_string(),
        }),
```

### Causa — **PROVEN** (mecanismo) + **HIPÓTESE** (errno exato)

1. **PROVEN:** PATH-skip faz o child **herdar** o PATH enorme do runner GHA (~centenas de entradas) em vez de `PATH=tmpdir`. Antes do fix, `command.envs(env)` forçava `PATH=tmpdir` (pequeno) → ARM64 verde.
2. **PROVEN:** O fix de python3 (evitar re-injetar PATH gigante no spawn absoluto) introduziu regressão no cenário de teste com shim isolado.
3. **HIPÓTESE:** O spawn falha com E2BIG/ENOENT/ETXTBSY por PATH herdado, mas o mapeamento `Err(_)` → `DependencyMissing` oculta o errno real.

### Proposta de correção

O skip de PATH do PR #39 trata o sintoma (PATH gigante no `execve`) e quebra o contrato de isolamento. No Linux, **uma** string de ambiente > 128 KiB (`MAX_ARG_STRLEN`) faz `execve` devolver `E2BIG` — inclusive com binário absoluto, porque o `envp` inteiro entra na conta ([LKML](https://lore.kernel.org/lkml/202310170957.DF7F7FE9FA@keescook/T/), [ninja#1261](https://github.com/ninja-build/ninja/issues/1261)). Omitir PATH **não** encurta: o filho herda o PATH enorme do runner.

Go `os/exec`, Cargo e o próprio `runtime.rs` (`command_for_script_with_env`, task 1751) fazem: resolver no PATH injetado → `Command::new(absoluto)` → **definir** o PATH do filho de propósito.

Política única (todos os SO):

```text
PATH_child =
  se o caller injetou PATH e cabe num teto seguro (ex. 32 KiB): usar o injetado
  senão: dirname(binário) + PATH mínimo do SO
```

Mínimo por plataforma (o que systemd/containers usam quando querem PATH determinístico):

- Unix: `/usr/bin:/bin`
- macOS: `/usr/bin:/bin:/usr/sbin:/sbin`
- Windows: `%SystemRoot%\System32` + diretório do `git.exe` (continua evitando o `bash.exe` do WSL)

|Ação|Detalhe|
|---|---|
|PATH explícito no spawn absoluto|**Nunca** omitir (= herdar). Injetado se for curto; senão mínimo. Nunca `inherited_PATH + shim_dir`|
|Preservar o fix python3|PATH GHA enorme deixa de ser reaplicado (cai no mínimo). `PATH=tmpdir` do teste é curto → **é** aplicado → ARM64 isola de novo|
|Erro real em logs/testes|Em `ensure_command_output`, propagar `io::Error` (pelo menos em `cfg(test)`). `E2BIG`/`ETXTBSY`/`ENOEXEC` não viram `DependencyMissing`|
|Teste local de PATH enorme|No mesmo teste: PATH injetado curto (shim) **e** PATH sintético com ~200 entradas / string > 128 KiB|
|**Não fazer**|Reverter o PATH-skip por inteiro (reintroduz `python3` no gnu); remover o teste de PATH injetado|

---

## Por que `check:full` local não pega

| Gap | `check:full` local | CI (`ci.yml` matriz) |
|-----|-------------------|----------------------|
| Plataformas | Apenas `linux-gnu` host (linha 54–55 de `scripts/tasks/check/full`) | 9 jobs: Linux gnu/musl × x86_64/aarch64, macOS × 2, Windows × 2 (linhas 17–65 de `.github/workflows/ci.yml`) |
| Windows battery-add | Path Linux ≈ canônico → probe passa | `canonicalize` + `\\?\` strip diverge de `to_str()` |
| ARM64 PATH | Host x86_64 Arch; teste roda mas sem PATH GHA enorme | `ubuntu-24.04-arm` com PATH do Actions (~KB) |
| python3 PATH | PATH local moderado | PATH GHA massivo quebra spawn com PATH re-injetado (corrigido, mas expôs regressão ARM64) |
| askpass ETXTBSY | Teste roda; flake raro em FS local | Paralelismo + overlay do runner amplia race |
| Bash 3.2 / nounset | Linux Bash 5.x | macOS Bash 3.2 com `set -u` |
| musl static | Não executado localmente | Target `*-unknown-linux-musl` dedicado |

Os três testes **já rodam** no `check:full` (`test-lib` + `behavioral_adapter_parity` via `scripts/tasks/suite/native-tests`) e passam no host porque afirmam a superfície do SO, não a invariante. Não é “faltou Windows no laptop” — é fixture incompleta.

| Falha na CI | Roda no `check:full` hoje? | Por que passa no host | O que o teste local precisa para falhar *aqui* |
|-------------|----------------------------|------------------------|------------------------------------------------|
| Windows `mismatch.battery-add` | Sim (`cargo test --test behavioral_adapter_parity`) | `tempfile` + `canonicalize` no Linux costumam dar a mesma string que `to_str()` | Fixture `repo-link` → `repo-real`; probe compara `normalized_local_git_url` |
| gnu askpass ETXTBSY | Sim (`cargo test --lib git_askpass_refuses_credentials_for_another_host`) | FS local raro; `.unwrap()` esconde errno | Helper install-then-exec; exec imediato; falha com `io::Error`, não `unwrap`; stress `--test-threads=32` |
| ARM64 PATH / `DependencyMissing { git }` | Sim (`cargo test --lib test_dependency_checks_use_injected_path_for_every_runtime`) | PATH do laptop é curto; skip omite PATH e o filho herda o do processo (ainda pequeno) | No mesmo teste: PATH injetado curto **e** PATH sintético enorme / string > 128 KiB; filho nunca herda |

`linux-musl` no CI também **não** executa a suíte ligada a musl: `native-tests` no host gnu + build estático + `omakure --version`. `check:full` nem chama `linux-musl`.

O laptop **não** substitui: Windows `\\?\` real, `ubuntu-24.04-arm` com PATH do Actions, macOS Bash 3.2 + `set -u`, overlayfs + paralelismo do runner. Não há `cross`/`act`/qemu-user nos scripts. Isso é confirmação de plataforma — não descoberta de invariante.

---

## Propostas de prevenção

A matriz CI cobre portabilidade (SO, arquitetura, toolchains). Invariantes que se expressam em linux-gnu — comparação de URL normalizada, PATH do filho, install-then-exec sem ETXTBSY — precisam de teste no host **no mesmo PR** que o conserto. `check:full` já executa `test-lib` e `behavioral_adapter_parity`, mas com fixtures incompletas os três casos passam até o PR adicionar o fixture certo.

**Regra:** se a invariante se expressa em linux-gnu, o teste entra em `test-lib` ou `behavioral_adapter_parity` **no mesmo PR do conserto**. `mise run check:full` (ou, no mínimo, `mise run test:unit` + o probe de add) tem que falhar *antes* do fix e passar *depois*. A matriz CI não é o primeiro lugar em que a regressão aparece.

| Invariante | Falha que a viola | Primativa no teste local |
|-----------|-------------------|--------------------------|
| Identidade persistida = forma que o teste compara | Windows `mismatch.battery-add` | `normalized_local_git_url` em todo probe de path |
| Filho nunca herda PATH acidental; arquivo gerado já nasce no modo final | ARM64 PATH + gnu ETXTBSY | PATH explícito e limitado no spawn; `open(tmp, modo_final)` → close → rename → exec |

| Prática | Detalhe |
|---------|---------|
| Teste no mesmo PR do fix | Invariante em `test-lib` ou `behavioral_adapter_parity`; gate local deve falhar antes e passar depois do conserto |
| Fixture de invariante no host, não só na matriz | Path não canônico, PATH enorme, install-then-exec — sem isso o `check:full` volta a passar e o CI volta a ser o detector |
| Propagar errno real em `cfg(test)` | `DependencyMissing` genérico e `.unwrap()` no askpass escondem ETXTBSY/E2BIG |
| CI para portabilidade, não descoberta | Windows real, ARM64 GHA, Bash 3.2, musl link — confirmam plataforma após invariante verde no host |

### Gate local — rodar antes do push

`check:full` chama só `scripts/tasks/check/platform/linux-gnu` (host). Os três casos abaixo já estão nessa suíte. Depois dos fixtures do PR, estes comandos têm que falhar sem o conserto e passar com ele — **sem esperar a matriz**.

Comandos que existem hoje (ainda podem passar até os fixtures entrarem):

```bash
# Askpass (test-lib — já no check:full)
cargo test --lib git_askpass_refuses_credentials_for_another_host --locked -- --test-threads=32

# PATH do filho (test-lib). PATH enorme simula GHA; o teste ainda precisa *afirmar* que o filho não herda.
PATH="$(python3 -c 'print(":".join(["/tmp/p"]*300))'):$PATH" \
  cargo test --lib test_dependency_checks_use_injected_path_for_every_runtime --locked

# Probe de add (native-integration — já no check:full). No Linux passa até existir symlink não canônico.
cargo test --test behavioral_adapter_parity battery_add --locked
```

Atalhos mise (o que o desenvolvedor já usa):

```bash
mise run test:unit          # test-lib (askpass + PATH)
mise run test:integration   # inclui behavioral_adapter_parity (add)
mise run check:fast         # fmt/clippy + test-lib — não inclui o probe de add
mise run check:full         # suíte nativa gnu + certs; ainda só host linux-gnu
```

Musl local (opcional, não está no check:full): precisa `musl-gcc`.

```bash
scripts/tasks/check/platform/linux-musl x86_64-unknown-linux-musl
```

Isso é build estático + ELF + `--version`. Não executa `cargo test` sob musl. O ETXTBSY gnu/musl se pega no `test-lib` do host.

O que **não** tentar reproduzir no Arch: runner Windows, ARM64 GHA, Bash 3.2. Essas células confirmam plataforma. Se a invariante não tiver teste no host, o PR não está pronto.

### O que NÃO fazer

- Não deixar a matriz CI ser o primeiro detector de uma invariante que linux-gnu consegue expressar (symlink, PATH sintético, install-then-exec).
- Não considerar “passa no check:full” suficiente se o teste não força path não canônico / PATH enorme / exec imediato pós-install.
- Não gravar URL crua da CLI só para satisfazer probe desalinhado.
- Não reverter PATH-skip por inteiro (reintroduz `python3` no gnu).
- Não silenciar askpass com retry infinito ou helper persistente.
- Não remover testes de PATH injetado ou askpass host-bound.

---

## Arquivos-chave

| Arquivo | Papel |
|---------|-------|
| `scripts/tasks/check/full` | Gate local completo — só `linux-gnu` host |
| `scripts/tasks/check/platform/linux-gnu` | native-tests + build-release + binary-smoke |
| `scripts/tasks/check/platform/linux-musl` | Build estático musl (não no `check:full`) |
| `scripts/tasks/suite/native-tests` | test-lib + native-integration (inclui behavioral_adapter_parity) |
| `scripts/tasks/atomic/test-lib` | `cargo test --lib --locked` (askpass + PATH) |
| `.github/workflows/ci.yml` | Matriz 9 células — confirmação de plataforma |
| `tests/behavioral_parity/battery.rs` | Probes `battery-list` / `battery-add` |
| `src/adapters/system_checks.rs` | PATH do filho + `DependencyMissing` |
| `src/operations/battery.rs` | `prepare_git_askpass` + `normalize_git_url` |

[Showing lines 1-300 of 397. Use :301 to continue]
# Plano de refactoring — `refactor/project-review`

Plano único para implementar, nesta branch, todos os pontos levantados na
auditoria de 30/09/2026 (arquitetura, qualidade de código, testes, ferramentas,
CI e documentação). Cada fase é entregue como um ou mais commits
Conventional Commits (ver `CONTRIBUTING.md`); não há PRs intermediários.

## Metas obrigatórias

Ao final da branch:

- **Zero código morto:** nenhum `#[allow(dead_code)]`, nenhum item sem uso,
  nenhum diretório ou arquivo órfão.
- **Zero código legado:** nenhuma compatibilidade com nomes, formatos ou modos
  antigos que o produto atual não usa (ver Fase 1b).
- **Zero código duplicado:** nenhuma função auxiliar definida em mais de um
  lugar, em `src/` e em `tests/`.

## Regras gerais

- **Sem mudança de comportamento** nas fases de reorganização (2 a 7), exceto
  a remoção de legado da Fase 1b. Contratos JSON, rotas HTTP, códigos de erro
  e saída da CLI permanecem idênticos.
- Mover código e alterar código ficam em **commits separados**: primeiro o
  movimento puro (diff revisável como rename), depois a alteração.
- Cada commit precisa passar `mise run check:fast`.
- Fases que tocam PATH, instalação, overlay, symlinks, empacotamento ou CI
  terminam com `mise run check:full`.
- O gate de complexidade é um ratchet: nenhuma função tocada pode piorar.
  Funções refatoradas devem melhorar o baseline em `quality/complexity/`.
- O gate de cobertura (`scripts/coverage/baseline.json`, ~86,8%) não pode cair.
- Ao final de cada fase, marcar os itens concluídos neste arquivo.

## Linha de base (antes de começar)

- `check:fast`: verde, 1.249 testes de biblioteca, ~32 s.
- Maiores arquivos: `cli/api.rs` 6.406, `node_registry.rs` 6.281,
  `operations/battery.rs` 5.224, `direct_service.rs` 5.086,
  `node_registry/health.rs` 3.624, `runs.rs` 3.489, `operations/node.rs` 3.120,
  `node.rs` 2.786 linhas.
- Piores funções (ciclomática / cognitiva): `direct_service::hold_session`
  49/95, `policy::allows` 47/34, `node_registry::health::evaluate` 38/41,
  `discovery_loop` cognitiva 51, `domain::node_config::validate` 30/34,
  `run_executor::execute_with_heartbeat_guarded` 29/30 (303 linhas).

- CI completo verde no commit de partida `b418789` (PR #50): 8 plataformas,
  cobertura, complexidade, empacotamento, docker-smoke e certificações.
- Complexidade e cobertura de partida: `quality/complexity/baseline.json` e
  `scripts/coverage/baseline.json` em `b418789`.

---

## Fase 1 — Limpeza e documentação

Baixo risco, remove ruído antes das mudanças grandes.

- [ ] Apagar os diretórios vazios `src/adapters/tui/` e
      `scripts/tasks/__pycache__/`.
- [ ] Adicionar `__pycache__/` e `*.pyc` ao `.gitignore`.
- [ ] Confirmar que `dirs` não é usado (`rg 'dirs::' src`) e removê-lo do
      `Cargo.toml`; atualizar as listas de dependências nas docs.
- [ ] Revisar os 8 `#[allow(dead_code)]` (`search_index.rs:27,121`,
      `cli/queue.rs:526`, `cli/api.rs:714,904`, `auth.rs:540`, `runs.rs:108`,
      `ports/environment.rs:7`): remover o código morto ou justificar.
- [ ] `AGENTS.md`:
  - [ ] Corrigir `mise run dev:smoke` para `mise run dev` (ou renomear a task
        para `dev:smoke` no `mise.toml`, escolhendo um nome e removendo o
        duplicado `scripts/tasks/dev/smoke` vs `scripts/tasks/atomic/dev-smoke`).
  - [ ] Substituir a árvore de arquitetura parcial por um resumo dos planos
        da frota (node registry, direct transport, health plane, remote cue,
        baseline, enrollment, discovery) com ponteiro para
        `docs/internal/architecture.md` como fonte canônica.
  - [ ] Atualizar a lista de dependências mantidas (`k256`, `snow`,
        `hickory-resolver`, `curve25519-dalek`, `serde_jcs`, `tempfile`,
        `libc`, `windows-sys`; remover `dirs`).
- [ ] `docs/internal/architecture.md`: completar a tabela de stack com as
      dependências faltantes.
- [ ] Corrigir o título em português `## Referência` no `README.md`/docs para
      manter o idioma consistente.

Validação: `check:fast`.

---

## Fase 1b — Remoção de legado

Muda comportamento de propósito. Ferramentas do repositório que dependam de
algum item são migradas antes da remoção.

- [x] Nomes antigos do produto: variáveis `OVERTURE_*` e `CLOUD_MGMT_*` e
      diretórios padrão antigos (`overture-scripts` etc.) em `main.rs`,
      `operations/config.rs` e `cli/update.rs`.
- [x] Migração da v0.1 em `runs.rs`: `rebuild_legacy_schema_if_needed` e
      `cleanup_legacy_json_files`.
- [x] Modo de token legado `OMAKURE_API_TOKEN` (`auth.rs`, `cli/api.rs`):
      apenas tokens Argon2id via `--tokens-file`.
- [x] Migrações antigas do schema do `node.sqlite` em `node_registry`:
      aceitar só o schema atual.
- [x] Qualquer outro caminho marcado como legado, compatibilidade ou
      deprecated que o produto atual não use.
- [x] Atualizar docs, help do clap, `help-ai`, referência da CLI e testes.

Validação: `check:full`.

---

## Fase 2 — Helpers compartilhados no código de produção

Eliminar cópias antes de dividir os módulos, para não multiplicar o trabalho.

- [ ] Criar `src/cli/emit.rs` com `emit_error` e `emit_operation_error`;
      substituir as 9 e 5 cópias nos módulos da CLI.
- [ ] Criar `src/util/hex.rs` (ou `crate::encoding`) com `hex` e `decode_hex`;
      substituir as 7 e 4 cópias.
- [ ] Centralizar `logical_relative_path` (5 cópias) em `operations/path.rs`.
- [ ] Unificar `write_atomic*` (battery e node), `constant_time_eq` (auth e
      discovery) e `parse_duration_*` (queue e history).
- [ ] Transformar `util.rs` em `util/mod.rs` com submódulos coesos
      (`exec`, `fs`, `hex`, `time`).

Validação: `check:fast`.

---

## Fase 3 — Divisão dos módulos grandes (movimento puro)

Cada arquivo vira um diretório de submódulos. Commits de movimento sem
alterar lógica; testes inline vão para `tests.rs` do próprio módulo
(`#[cfg(test)] mod tests;`).

- [ ] `src/cli/api.rs` → `src/cli/api/`
      `{mod, boot, auth, policy, types, router, audit, tests}` +
      `handlers/{health, scripts, envs, runs, node, battery, secrets}`.
- [ ] `src/direct_service.rs` → `src/direct_service/`
      `{mod, error, admission, resolver, session, cue, baseline, enrollment,
      status, tests}`.
- [ ] `src/node_registry.rs` → `src/node_registry/`
      `{mod, types, error, open, peers, enrollment, audit, migrate,
      schema_validate, locks, tests}` (mantendo `health/`).
- [ ] `src/node_registry/health.rs` → `src/node_registry/health/`
      `{mod, types, apply, evaluate, outbox, feed, prune, audit, rows, tests}`.
- [ ] `src/operations/battery.rs` → `src/operations/battery/`
      `{mod, types, registry, sync, install, manifest, git, path_safety,
      fs_unix, tests}`.
- [ ] `src/runs.rs` → `src/runs/`
      `{mod, state, schema, open, query, enqueue, lifecycle, trace, ids, tests}`.
- [ ] `src/operations/node.rs` → `src/operations/node/`
      `{mod, status, trust, enrollment, bundle, discovery, errors, tests}`.
- [ ] `src/node.rs` → `src/node/`
      `{mod, layout, context, private_token, fs_unix, fs_windows, policy, tests}`,
      concentrando os ~30 blocos `unsafe` em `fs_unix`/`fs_windows`.
- [ ] Avaliar a mesma divisão para `cli_http_parity.rs`, `direct_transport.rs`,
      `remote_cue.rs` e `cli/args.rs` (1,9k–2k linhas cada).
- [ ] Atualizar `tests/architecture_contract.rs` se ele referenciar caminhos
      de arquivos.

Validação: `check:fast` a cada módulo; `check:full` ao final (Windows/Unix
`cfg` mudam de arquivo).

---

## Fase 4 — Redução de complexidade

Agora com arquivos menores, refatorar as funções mais complexas. Cada uma
ganha testes de caracterização antes da mudança, se ainda não tiver.

- [ ] `direct_service::hold_session` (49/95): extrair a máquina de estados da
      sessão em tipo próprio com transições pequenas; remover o
      `too_many_arguments`.
- [ ] `policy::allows` (47/34): tabela de regras ou funções por tipo de regra.
- [ ] `node_registry::health::evaluate` (38/41, ~247 linhas): separar
      avaliação por tipo de sinal.
- [ ] `discovery_loop` (cognitiva 51, 12 parâmetros).
- [ ] `domain::node_config::validate` (30/34): validadores por seção.
- [ ] `run_executor::execute_with_heartbeat_guarded` (303 linhas) e
      `cli/node_service::run` (288 linhas): extrair etapas nomeadas.
- [ ] `node_registry::validate_schema` (~253 linhas) e
      `stage_manual_enrollment` (~227 linhas).
- [ ] `operations::node::apply_signed_bundle_with_actor` (~210 linhas) e
      `map_registry_error` (ciclomática 24).
- [ ] `operations::battery::reject_unsafe_git_config_text` (25/25) e
      `install_battery_script` (~145 linhas).
- [ ] Objetos de parâmetros para retirar os 18
      `#[allow(clippy::too_many_arguments)]`, começando por
      `enrollment::sign_with_material` (14), `enrollment_authority::issue` (13),
      `router_with_transport` (12) e `health_enqueue_signal` (10).

Validação: `check:fast`; baseline de complexidade atualizado e melhor.

---

## Fase 5 — Erros tipados

- [ ] `runs`: criar `RunsError` com `thiserror` e substituir as 22 assinaturas
      `Result<_, String>`.
- [ ] `search_index`: criar `SearchIndexError`.
- [ ] Demais `Result<_, String>` (~44 restantes em `cli/serve*`,
      `run_executor`, `secrets`, autostart Windows).
- [ ] Mapear erros de módulo para `OperationError` somente na fronteira de
      `operations/`; unificar os códigos do envelope da CLI
      (`cli/json.rs:31-39`) com `OperationErrorCode`.
- [ ] Restringir `AppError` (`error.rs`) a schema/ambientes ou absorvê-lo.
- [ ] Revisar os ~78 `unwrap`/`expect` de produção (piores: `cli/node.rs`,
      `direct_transport.rs`, `auth.rs`, `cli/api`, `node_registry.rs`):
      trocar por erro propagado ou documentar a invariante no `expect`.

Validação: `check:fast`; conferir que os códigos de erro JSON/HTTP não
mudaram (testes de contrato e paridade).

---

## Fase 6 — Fronteiras de arquitetura

- [ ] Quebrar o ciclo `health_plane` ↔ `node_registry`: tipos de fio e de
      domínio ficam em `health_plane/{model, schema, bounds}`; aplicação e
      persistência passam por uma interface `HealthStore` implementada em
      `node_registry`.
- [ ] Criar `operations::cue` e `operations::baseline` e fazer os handlers
      `node_cue_handler`, `node_baseline_handler` e rollback chamarem
      operações em vez de `remote_cue`, `direct_service` e `baseline_push`.
- [ ] Mover `install_signal_handlers` e `worker_loop` de `cli/queue.rs` para
      `adapters/signals` e `operations/worker`; remover a exceção em
      `tests/architecture_contract.rs:60-71`.
- [ ] Extrair `command_inventory` e `HTTP_ROUTE_INVENTORY` para
      `crate::inventory`, para que `cli_http_parity` e `operation_catalog` não
      dependam de `cli::*`.
- [ ] Fazer `operations/core.rs` e `operations/health.rs` pararem de receber
      `&rusqlite::Connection`; expor consultas de `runs` que retornem DTOs.
- [ ] Remover `use_cases/`: incorporar `EnvironmentService` em
      `operations/envs`; manter em `ports/` só interfaces com mais de uma
      implementação real ou com fakes em testes.
- [ ] Mover a execução de `git` de `operations/battery` para `adapters/git` e
      os blocos `unsafe` de sistema de arquivos para `adapters/fs`.
- [ ] Reduzir a superfície pública de `lib.rs` (`pub(crate)` onde os testes
      de integração não precisam).
- [ ] Adicionar as novas regras ao `tests/architecture_contract.rs`
      (sem ciclo health/registry, sem `cli::` fora de `cli/`, sem
      `std::process::Command` em `operations/`).

Validação: `check:fast`; `check:full`.

---

## Fase 7 — Async sem bloqueio

- [ ] Envolver em `tokio::task::spawn_blocking` (ou num helper único
      `blocking_operation`) os ~28 handlers que chamam operações síncronas ou
      SQLite, começando por `list_runs_handler` e `enqueue_run_handler`.
- [ ] Teste que exercite requisições concorrentes contra um handler lento
      para provar que o runtime não trava.

Validação: `check:fast`; `mise run test:node-service`.

---

## Fase 8 — Testes

- [ ] Levar para `tests/support/` os helpers duplicados: `run_node` (8×),
      `assert_success` (8×), `hex` (9×), `trust_peer`, `init_node`, `serve`,
      `read_frame`, `node_material`, `profile_payload`, `omakure(_with_env)`.
- [ ] Criar `tests/support/docker.rs` com `compose` e `wait_for_health`.
- [ ] Fazer os 18 binários que não usam `support` passarem a usá-lo.
- [ ] Trocar as pausas fixas por espera de condição:
      `remote_cue_e2e.rs:331,383,577,650`, `direct_transport_e2e.rs:1439,1517`,
      `health_plane_transport_e2e.rs:456`.
- [ ] Documentar ou tornar configuráveis as portas fixas
      (`17878`/`17879` do Compose, `38383` em `behavioral_parity/node.rs`).
- [ ] Dividir os maiores testes em etapas nomeadas com fixture compartilhada:
      `health_plane_contract.rs:1830` (~669 linhas),
      `docker_health_plane_adversary.rs:737` (~589),
      `docker_signed_bundle_e2e.rs:637` (~374),
      `health_plane_transport_e2e.rs:1001` (~325).
- [ ] Usar `rstest` ou tabelas nas matrizes adversariais em `tests/`.
- [ ] Agrupar binários de integração por domínio (ex.: `health_plane_*`,
      `docker_*`, CLI/HTTP/node) reduzindo os 33 binários; atualizar em
      conjunto `scripts/tasks/suite/native-integration` e a verificação em
      `packaging_smoke.rs:1024-1072`.
- [ ] Tornar explícito (em vez de silencioso) o skip dos crates
      `#![cfg(unix)]` no Windows.
- [ ] Aumentar a cobertura de `cli/update.rs` (~40%),
      `cli/serve_autostart.rs` (~41%), `cli/uninstall.rs` (~54%) e
      `installer.rs`, ou documentar o que é exclusivo de plataforma.
- [ ] Avaliar mover os testes com TCP real de `direct_service` para
      `tests/` se o tempo do `check:fast` crescer.
- [ ] Subir o baseline de cobertura se ele melhorar.

Validação: `check:fast`; `check:full` (suítes Docker e certificações).

---

## Fase 9 — Dependências e toolchain

- [ ] Adicionar `rust-toolchain.toml` com `1.97.1`, igual ao `mise.toml` e
      aos workflows.
- [ ] Adicionar tabela `[lints]` no `Cargo.toml` com as regras hoje implícitas
      (clippy `-D warnings`, `unsafe_op_in_unsafe_fn`, etc.).
- [ ] Atualizar `thiserror` 1 → 2.
- [ ] Atualizar `rand` 0.8 → 0.9 (reduz duplicatas de `rand_core`/`getrandom`).
- [ ] Atualizar `axum` 0.7 → 0.8 (sintaxe de rotas `/{param}`).
- [ ] Atualizar `rusqlite`, `toml`, `winreg` e `mlua` para as versões atuais.
- [ ] Rodar `cargo tree -d` e registrar as duplicatas restantes
      (`sha2` 0.10/0.11 via `k256`).
- [ ] Registrar como revisar o pin `rev = "9732c63"` de `jdx/usage`.
- [ ] Migrar para edition 2024 (último item, commit isolado).

Validação: `check:full` após cada atualização principal.

---

## Fase 10 — CI e scripts

- [ ] Adicionar `Swatinem/rust-cache` aos jobs de CI e release.
- [ ] Separar "testar" de "compilar release + smoke" na matriz de 8
      plataformas; células musl/cross não repetem a suíte e2e completa.
- [ ] Fazer o workflow de release reaproveitar artefatos do CI ou usar um
      caminho só de empacotamento, sem repetir a matriz de testes.
- [ ] Mover `cli-reference --check` para o job `usage-artifacts`.
- [ ] Concentrar as etapas de complexidade (CI e `complexity-soak.yml`) num
      único script.
- [ ] Remover entrypoints duplicados de tasks (`usage-docs` vs
      `atomic/usage-docs`, `dev/smoke` vs `atomic/dev-smoke`).

Validação: `check:full`; push da branch e CI verde em todas as plataformas.

---

## Fechamento

- [ ] Atualizar `AGENTS.md` e `docs/internal/architecture.md` com a nova
      árvore de módulos resultante das fases 3 e 6.
- [ ] Comparar com a linha de base: tamanho dos maiores arquivos,
      complexidade, cobertura, tempo de `check:fast` e de CI.
- [ ] `mise run check:full` verde.
- [ ] Remover este arquivo (ou movê-lo para `docs/internal/`) antes do merge.
- [ ] Abrir o PR único contra `master`.

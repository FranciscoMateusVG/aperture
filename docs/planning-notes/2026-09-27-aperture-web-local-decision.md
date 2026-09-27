# Aperture Web Local — documento de decisão (rev. 2)

**Bead:** aperture-v3eqx · **Autor:** Wheatley · **Status:** revisão 2 após ROOT REVIEW (GLaDOS, 35crof) e decisão do operador (7zmeyk: **browser-only, Tauri retirado** após ciclo completo e atualizações validados). Documento em revisão; **nada aqui autoriza implementação, migração, instalação ou operação live.** Respostas item a item ao root review no Apêndice E.

**Pins reais (verificados 2026-09-27):** `origin/master` = `2703dc4` (PR #71; **não contém o runtime V4**) · instalada = `5394ae0` · root = `d6d01cc` · **head cumulativo = `abeb9ae`** (freeze diagnóstico Peppy, descendente de `d6d01cc` ← `5394ae0`; revisão independente PASS). Linhas citadas são `@5394ae0` salvo indicação; `abeb9ae` só acrescenta diagnóstico em `team_claude_launch.rs`/`team_replacement_native.rs`.

---

## 0. Resumo executivo

- **Problema** (operador, 27/set): confiabilidade dos times e ciclo de atualização/teste da interface. Hoje: cada mudança de UI exige rebuild + reinstalação do app Tauri; os daemons de runtime (hub, app-servers Codex, watchdog, poller) vivem **dentro** do processo da GUI e morrem com ela (`lib.rs:282-284` mata hub e app-servers ao sair); a autoridade do operador é o próprio canal IPC (`AuthenticatedActor::operator_ui()` cunhado incondicionalmente, `teams.rs:1897…2354`).
- **Decisão de destino (operador):** UI **no navegador**, servidor **local** no Mac, **Tauri retirado** ao fim do ciclo. Tauri permanece só como fallback temporário e **nunca como segundo controller ativo**.
- **Recomendação técnica:** `aperture-server` — binário Rust no mesmo crate, reutilizando `aperture_lib` (inventário concreto no Apêndice A: 58/64 arquivos sem nenhum símbolo Tauri; 4 arquivos com wrappers; 21 comandos registrados, 4 anotados sem registro) e assumindo os quatro daemons com **ownership explícito** (lock de controller único + fatos de identidade por daemon + adoção provada por fixture) — não "adoção por porta". Transporte loopback com **dois princípios distintos** (operador ≠ GLaDOS; nenhum bearer concede autorização global), rotas só para as ações que hoje o operador já pode executar pela GUI. Fronteira de release versionada que desacopla o checkout vivo dos seats ativos.
- **Resultado aceito deste ciclo (obrigatório, não opcional):** (1) ciclo completo de time com **READY/BEADS reais** (criar → aprovar → bootstrap 2×Codex → mensagens → falha → recovery → parar → archive) sob o servidor; (2) diagnóstico durável e falha cedo (freeze `abeb9ae`); (3) **inspeção em quarentena e sessão tmux por time** (com as correções C1–C5); (4) restart de UI/servidor sem afetar workers, provado; (5) atualização de UI/servidor sem reinstalação; (6) retirada do Tauri após (1)–(5) validados. Política de merge de produto **fica fora**.
- **Custo:** 14–20 dias-agente (estimativa, não prazo), time 2×Codex ainda não criado, sequencial após F0.

---

## 1. Prior art (citado, não repetido)

| Fonte | O que já resolve | Tratamento |
|---|---|---|
| `docs/MIGRATION-WEB-APERTURE.md` (`7edfc2b`, 2026-05, em master) | "Manter Rust, trocar IPC Tauri por Axum" | Tese central reaproveitada; **supersedido** em alvo (Mac local), exposição (`127.0.0.1`), CORS (nenhum), módulos inexistentes (`warroom/spawner/xterm`), e agora **convergente** na retirada do Tauri (decidida pelo operador). Será arquivado com nota de substituição na F0. |
| `docs/runtime-v4-safety-contract.md` §131-198 | Freeze cumulativo, fixtures isolados, preimages, helpers pareados, rollback ≠ apagar journal | Base do procedimento de F2 — **complementado** (não "reusado integralmente") com o lock de controller e o handoff exclusivo (§7), que o plano antigo não tem. |
| `docs/runtime/*.md` (11) | Contratos vigentes | Mantidos; `managed-terminal.md` e `installer-publication.md` mudam na F4/F5. |
| Epic 4rsnc — retrospectiva | TOP P1 readiness hello; não construir scheduler/broker/polling global/novo canal/E2E amplo/novo harness | P1 readiness entra como parte do ciclo obrigatório (READY real); vetos respeitados. |
| k7y2s | Gate perde stderr; diagnóstico `abeb9ae` PASS; plano inspeção/tmux-por-time PARKED com C1–C5 | Diagnóstico = REUSAR; inspeção/tmux-por-time = F4 **obrigatória**. |
| `tests/boot-harness` (xt16e) | L1/L2/L3, sessão tmux efêmera, stubs, hub isolado | Base dos testes sintéticos e do witness. |
| `tests/real-bd-team-seat.test.mjs` | `bd` real contra banco fixture | Base do "READY/BEADS real" em ambiente isolado. |

---

## 2. Estado atual — fatos com fonte

**Processo e ciclo de vida.** `lib.rs::run()` (`:166-284`) sobe no processo da GUI: `poller` (thread 5s, badges em memória), `ws_hub` (filho `node ws-hub.js` em `127.0.0.1:4517`; respawn 2s; **mata com `kill -9` qualquer pid que escute a porta**, `ws_hub.rs:70-80,208-223`), `codex_appserver` (um filho `codex app-server --listen unix://…/<seat>.sock` por agente Codex; `spawn_app_server` **só faz `connect()` e retorna** quando o socket já responde — não adota supervisor, identidade nem bridge, `codex_appserver.rs:92-103`; `shutdown()` mata todos e apaga os sockets `:227-240`), `watchdog` (subscriber ao hub, `decision_loop` 2s com re-kick por `send-keys`/respawn, varredura `bd` 45s; estado só em memória `Shared`, `watchdog.rs:128,462`). Único hook de encerramento: `RunEvent::Exit`. **Sobrevivem à GUI:** filhos managed do launch gate (`setsid`, `team_launch_gate.rs:79-104`) e panes tmux. **Morrem:** os quatro daemons, badges, presença em memória, `AppState.team_preparations` (permits de replacement).

**Consequência hoje ignorada:** fechar o app mata os app-servers dos agentes de coordenação Codex (GLaDOS/Peppy) — o pane `codex --remote` perde o servidor. Qualquer handoff precisa tratar isso explicitamente (§7).

**Acoplamento Tauri.** Apêndice A. `tauri::` fora de assinaturas de comando só em `lib.rs` (builder, `generate_context`, `RunEvent`), `teams.rs` (2 helpers privados sobre `tauri::State`) e `team_terminal.rs` (`async_runtime::spawn_blocking`). Superfície headless já existente: `managed_claude_gate/observe`, `attach_managed_terminal`, `boot_agent_headless`, `team_control_json`; bins `aperture-boot`, `aperture-team-control`.

**Autoridade.** Dois princípios já existem no código e **não se confundem**: `operator_ui()` (GUI) e o ator GLaDOS autenticado (`authenticate_glados_control`, MCP → `aperture-team-control`). Matriz em §4.3.

**Frontend/testes.** Apêndice B. **Instalação/árvore.** Apêndice C.

---

## 3. Alternativas (comparação pequena)

Critérios: (a) independência de ciclos UI ↔ servidor ↔ workers; (b) reuso das correções revisadas; (c) contrato local de autoridade sem shell e sem bearer global; (d) testabilidade sintética + witness real; (e) reversibilidade com um só controller; (f) custo.

| | A. Manter Tauri, corrigir só o ciclo de UI | **B. `aperture-server` Rust sobre `aperture_lib`** | C. Servidor Node chamando bins headless | D. Node UI + daemon Rust por socket |
|---|---|---|---|---|
| (a) | ✗ daemons continuam na GUI | ✔ daemons no serviço com ownership explícito | ◐ daemons são Rust ⇒ precisa do daemon Rust mesmo assim | ✔ |
| (b) | ✔ | ✔ | ✗ só `team_control_headless` coberto; `prepare/start`, `agents`, badges sem entrada headless | ◐ |
| (c) | n/a | ✔ dois princípios, rotas finitas | ◐ duas superfícies | ✗ novo canal (vetado 4rsnc) |
| (d) | ✗ | ✔ | ◐ | ◐ |
| (e) | ✔ | ✔ lock de controller + handoff | ◐ | ✗ |
| (f) | baixo, não resolve | **médio** | médio-alto | alto |

**Escolha: B**, agora com destino browser-only decidido (A cai por definição).

---

## 4. Arquitetura alvo

### 4.1 Processos, ownership e ciclos de vida
```
browser (descartável) ──HTTP loopback + token operador──▶ aperture-server (launchd; controller ÚNICO por flock)
                                                          ├─ serve ~/.aperture/releases/current/ui
                                                          ├─ API: só ações do OPERADOR (matriz 4.3), DTOs atuais
                                                          ├─ daemons com FATO DE IDENTIDADE cada (pid+birth, no-replace):
                                                          │    hub (setsid), app-servers Codex (setsid), watchdog, poller
                                                          └─ AppState (permits) — processo de vida longa
GLaDOS/Peppy ──MCP──▶ aperture-team-control (bin; sem mudança) ──▶ filesystem/hub   ◀── mesmo lock de escrita já existente (owner/CAS)
tmux "aperture" (coordenação) + "apt-<team>" ◀── workers: filhos setsid + panes — independentes
```
**Controller único.** `~/.aperture/run/daemons.lock` (flock exclusivo, criado privado). Quem não obtém o lock **não sobe daemons nem executa ações mutantes**, e informa o detentor (pid+birth lidos do próprio arquivo). Vale para o servidor e para o app Tauri de fallback (quando reconstruído do mesmo head; até lá, coexistência = procedimento, §7).

**Identidade e adoção (não é "por porta").** Cada daemon gerado pelo servidor recebe um fato privado no-replace por incarnação: `~/.aperture/run/daemons/<nome>.json` = `{pid, start_time_us, kind, socket_or_port, spawned_by:{pid,start_time_us}, release_sha}`; o processo é criado com `setsid` (grupo/sessão próprios) e o plist launchd usa `AbandonProcessGroup=true`, de modo que SIGTERM/crash/`kickstart -k` do servidor **não alcançam** os daemons. Na subida, para cada fato existente: (1) identidade `(pid, start_time_us)` via `team_process::state` ⇒ `Same`? (2) prova de protocolo do próprio serviço — hub: `presence.json.hub_pid == pid` (o hub já escreve isso, `ws-hub.ts:143`) **e** um hello de `subscriber` autenticado com resposta de snapshot; app-server: `connect()` no socket **e** o `pid` do `LOCAL_PEERPID` do socket == fato (primitiva já existe, `team_terminal.rs:peer_pid`). Só com (1)∧(2) o servidor **adota**: registra o daemon como supervisionado-por-identidade (poll `Same/Gone`; **sem `waitpid`**, exit code NOT_OBSERVED) e respawna **somente em `Gone`**. Se a porta/socket está ocupada por um pid **diferente** do fato, ou sem fato: **não mata**; o servidor sobe degradado, expõe `daemons: {hub: "occupied_by_unknown pid=…"}` e o operador decide. Isto substitui o `kill -9` por porta de `ws_hub.rs:70-80,215-223`, que é **removido**.

**Watchdog sem turnos duplicados.** Estado de re-kick (`attempt`, `at`, mtime do `.kickoff`) passa a ser persistido em `~/.aperture/run/daemons/watchdog-state.json` (escrita atômica, leitura na subida); na reconexão vale a `RECONNECT_GRACE` já existente; nenhum `send-keys`/respawn antes de (a) grace, (b) leitura do estado persistido, (c) presença do hub adotado. Seats managed continuam isentos (`classify_managed_seat`, `watchdog.rs:632-641`).

**Sobrevivência — o que é provado e o que é limite.** Provado por fixture inerte (F2): servidor gera daemon-stub com `setsid` → grava fato → recebe SIGTERM/`kill -9`/`kickstart -k` → stub continua vivo → novo servidor adota por identidade+protocolo sem sinal → pid desconhecido na porta não é morto → `Gone` ⇒ respawn único. **Limites declarados:** (i) um hub/app-server gerado pelo **app Tauri atual** é filho dele e morre no `RunEvent::Exit` — **não** é adotável através do quit do app; a primeira transição (F2) inclui exatamente **um** restart de hub/app-servers na janela autorizada (agentes reconectam com replay; panes `codex --remote` de coordenação podem exigir reabertura pelo caminho Open existente); (ii) processos adotados não têm `waitpid`: saída não observada em exit code; (iii) `AppState.team_preparations` não sobrevive a restart (tratamento em §7).

### 4.2 Mapa REUSAR / ADAPTAR / REMOVER (concreto)
**REUSAR (sem edição):** 58 arquivos Rust NONE do Apêndice A (todos `team_*`, `owner`, `journal`, `hub_auth`, `launcher`, `agent_loader`, `config`, `state`, `watchdog`* , `poller`, bins), `mcp-server` inteiro (incl. `ws-hub.ts`, `team-control.ts`), freeze `abeb9ae`, `tests/*.mjs`, `tests/boot-harness`, `docs/runtime/*`. (*`watchdog.rs` ganha só a persistência de estado — listado em ADAPTAR.)
**ADAPTAR (edição mínima, arquivos nomeados):** `lib.rs` (extrair `daemons::start(lock)` de `run()`; novo `bin/aperture-server.rs`); `ws_hub.rs` (remover kill por porta; fato de identidade; `setsid`; adoção); `codex_appserver.rs` (fato por seat; adoção por identidade+`LOCAL_PEERPID`; `setsid`); `watchdog.rs` (estado persistido); `agents.rs`/`teams.rs`/`tmux.rs`/`team_terminal.rs` (wrappers → funções sobre `Arc<Mutex<AppState>>`; ator vem do middleware, nunca cunhado no handler); `team_claude_launch.rs` (**pós-freeze**, pen Peppy: `infra` da release em vez de `CARGO_MANIFEST_DIR`, `:1181,:1638`; alvo `-t apt-<team>` `:1377-1383`); `src/services/tauri-commands.ts` (injetável) + `team-commands/runtime/terminal.ts` (adaptador HTTP); `main.ts`; `vite.config.ts`; `justfile` (`release-build`, `ui-deploy`, `test-ui`); `docs/runtime/managed-terminal.md`, `installer-publication.md`.
**REMOVER (F5, após validação):** `tauri::Builder`/`main.rs` Tauri, `tauri.conf.json`, bundle, `scripts/publish-desktop-installer.py`, `@tauri-apps/*`, `docs/MIGRATION-WEB-APERTURE.md` (arquivado com nota).
**NÃO transportar:** warroom/spawner/xterm/CORS/`0.0.0.0`; broker/scheduler/novo canal; supervisor de CLI Claude; captura de stderr bruto; `select-pane -d` como read-only; `switch-client` em todos os clientes.

### 4.3 Transporte e autoridade — matriz ator/ação
Dois princípios, **nenhum bearer concede o outro**:
- **Operador (browser)**: token local em `Authorization: Bearer` ⇒ o middleware constrói `AuthenticatedActor::operator_ui()` **apenas** para as ações que a GUI já executa hoje com esse ator: `list_agents`, `start/stop/restart_agent`, `update_agent_model`, `clear_attention`, `get_version`, `tmux_create_session`, `tmux_select_window`, `team_get_catalog`, `team_list_presets`, `team_save_preset`, `team_create` (aceita operador, `teams.rs:925`), `team_list`, `team_cancel_pending` (`:1182`), `team_bootstrap_seat` (`:2092`), `team_prepare_replacement`/`team_start_replacement` (`:2288,:2354`, permit store), `team_archive` (`:2375`), `team_open_seat` — **19 rotas** (não 21: `get_version` é leitura pública local; os 4 verbos tmux anotados mas não registrados **não** viram rota).
- **GLaDOS**: continua **exclusivamente** por MCP → `aperture-team-control` (stdin JSON, `authenticate_glados_control`): `Approve/activate` (`:1243`, glados-only), `StopSeat`, `RetireSeat`, `ValidateCheckpoint`, `ClaudeInboxProbe`, `ClaudeStartupSmoke`, `ReconcileClaudeStartup`, `InspectRemote/ResolveRemote`, `Replace`, `Archive`, `RollbackArchive`, `SaveRepository`, etc. **Nenhuma dessas ganha rota HTTP.** O bin não muda.
- **Seats**: `authenticate_seat_control` — inalterado (MCP).

**Bootstrap de autenticação (decisão técnica, sujeita a revisão de segurança — Cipher — antes da F1):** o servidor cria `~/.aperture/run/operator.token` (32 bytes, 0600, `O_NOFOLLOW|O_EXCL`, rotacionado por subida). O browser obtém o token por **uma única leitura local**: `aperture-server open` abre `http://127.0.0.1:<porta>/#t=<token-de-troca-de-uso-único>`; a página troca esse valor por um token de sessão em memória via `POST /session` (o token de troca expira em 30 s e é consumido uma vez), e **apaga o fragmento** do histórico (`history.replaceState`). Nada durável em URL/argv/log/referrer; sem cookie (sem auth ambiente ⇒ sem CSRF); `Authorization` apenas em memória da aba; nova aba ⇒ novo `open`. Requisições mutantes exigem `Origin` ∈ {`http://127.0.0.1:<porta>`, `http://localhost:<porta>`} **e** `Host` correspondente; `Sec-Fetch-Site` ≠ `same-origin` ⇒ 403. Bind `127.0.0.1` só. **WebSocket omitido** nesta fase: a UI é polling e continua (3 s). Testes de fronteira **reais** (F1): requisição sem token/401, token de outra subida/401, `Origin` externo/403, `Host` de rebinding/403, token de troca reutilizado/410, rota de ação GLaDOS inexistente/404, nenhuma rota aceita argv/shell. Validação por strings **não** conta como evidência.

### 4.4 Fronteira de release e compatibilidade
`~/.aperture/releases/<head-sha>/{bin/aperture-server, bin/aperture-boot, bin/aperture-team-control, mcp-server/dist/**, ui/**, RELEASE.json}` + symlink `~/.aperture/releases/current`. Regras:
- **Servidor** lê `RELEASE.json` da sua própria pasta e passa `infra=<pasta>` ao seam `validate_record_at(…, infra)` já existente; `CARGO_MANIFEST_DIR` deixa de apontar para o checkout vivo (ADAPTAR pós-freeze). Assim os pins dos `LaunchRecord` (`mcp-server/dist/{index,hub-client}.js`, `mcp-server-sentry`) passam a referenciar a pasta de release, não o repo.
- **Retenção:** uma pasta de release é retida enquanto **qualquer** `LaunchRecord`/owner ativo a referencie (varredura RO de `managed/<seat>/gN/claude-launch.json` e `owner/*.json`); só é removível em ação explícita após archive desses seats. Atualizar o servidor **nunca** sobrescreve arquivos de uma release referenciada.
- **Helpers:** `~/.aperture/bin/aperture-boot` e `aperture-team-control` permanecem no caminho fixo exigido por `team_claude_launch.rs:1201` e `team-control.ts:121`, como **cópias** da release `current`, substituídas apenas em janela de release quando nenhum lançamento está na fase de gate (≤10 s) — seats já Active não re-verificam o hash do helper (o gate hasheia uma vez, antes do exec); qualquer caminho futuro que re-valide pins pós-Active (recovery/reprepare) falha fechado e exige preparação nova — declarado, não escondido.
- **UI:** `just ui-deploy` = build → `releases/<sha>/ui` → troca do symlink `current` → recarregar aba; sem restart. **Servidor:** `just release-build` (bins + mcp dist de um único head congelado, hashes em `RELEASE.json`) + `launchctl kickstart -k` (daemons sobrevivem/adotados, §4.1). **Contrato de compatibilidade:** `RELEASE.json.schema` para UI↔API (a UI recusa API de schema diferente com mensagem, não silêncio) e para bins↔fatos em disco (o servidor recusa subir sobre `daemons/*.json` de schema maior).

---

## 5. Requisitos → atendimento → evidência exigida

| Requisito (todos obrigatórios) | Atendimento | Evidência |
|---|---|---|
| Times confiáveis, ciclo completo, READY/BEADS reais | Runtime intocado + daemons com ownership + P1 readiness (hello real após MCP conectado, `managed-mcp-readiness.md`) | Witness F3 com 2 seats Codex **reais** (autorizado), `bd` real em fixture, READY observado no hub |
| Erro durável / falha cedo | `abeb9ae` REUSADO | Já provado (66/39/25; reprodução 29/29 + 64/64) — **não** prova a causa Fable |
| Inspeção de quarentena | F4 com C1–C5: read-only **por cliente** (não `select-pane -d`), só cliente identificado (não `switch-client` global), binding histórico com `window_id/pane_id` persistidos na criação, "sem pane" só após cleanup verificado | Herméticos + witness |
| Sessão tmux por time, coordenação separada | F4: `apt-<team>`; incumbentes sem relaunch/move/kill; alvo `-t` em `team_claude_launch.rs:1377-1383` | T1–T7 |
| Testes browser + sintéticos isolados | `pnpm test` (existente) + testes de handler/auth + **um** witness browser (Playwright) em L2 (stubs) e depois real autorizado | §6 |
| Atualização sem reinstalar | §4.4 | `ui-deploy` sem restart: hash da `index.html` servida muda, pids não |
| Migração reversível, um controller | lock + handoff exclusivo + release retida (§7) | Ensaio de handoff e de rollback em janela autorizada com preimages |
| Retirar Tauri | F5 após F3/F4 validados | Bundle removido só depois do witness; app antigo retido como preimage |

---

## 6. Fases, aceite mensurável, custo (estimativas em dias-agente, não prazo)

**F0 — Freeze cumulativo e auditoria de branches (root, ~1 d).** Head = `abeb9ae`. Inventário por **conteúdo** (não por ancestralidade; `git cherry abeb9ae <tip>`): branches com patches **não** presentes no head que precisam de decisão integrar/supersedir antes do branch de implementação: `aperture-k310b-runtime-safety` (64 patches — provavelmente base squash-integrada; confirmar por diff), `aperture-g4hku-constitution-pilot` (30), `aperture-trgpo-claude-presence-hooks` (27), `aperture-ztid5-dokploy-scope` (22), `aperture-a4ph5-infisical-handoff` (12), `aperture-zfmd5-runtime-dialogs` (9), `aperture-84bby-polish` (5), `aperture-zfmd5-teams-ui` (4), `aperture-faaso-v4-team-foundation` (4), `aperture-xt16e-*` (3), `aperture-syzem-v4-role-templates` (2), e 1 patch cada em `yvcnp-stop-mcp`, `yvcnp-older-outsiders`, `yvcnp-checkpoint-validation`, `k310b-engine-release`/`06lm6-engine-catalog`, `jar5i-boot-ci`, `docfix-dokploy-subdomains`, `4iz7v-profile-hygiene`, `337gb-claude-runtime`, `337gb-claude-kickoff`. As demais 21 branches V4 têm 0 patches fora do head (absorvidas sob outro SHA). Branch de implementação `aperture-v3eqx-server` **sobre `abeb9ae`**; `origin/master` só recebe docs até a integração final. Adicionar `pnpm test`/`just test-ui`. Arquivar `MIGRATION-WEB-APERTURE.md` com nota. *Aceite:* head assinado, lista de branches classificada, `cargo test --lib` + `node --test` verdes, hashes registrados.

**F1 — `aperture-server` + autoridade + adaptador (4–6 d).** Bin novo; `daemons::start(lock)`; middleware operador (token de troca + sessão em memória; Origin/Host); 19 rotas 1:1 com DTOs atuais; ator **do middleware**; static `releases/current/ui`; `tauri-commands.ts` injetável + adaptador HTTP; app Tauri **inalterado** (mesmos módulos, mesmo build). Revisão de segurança (Cipher) do bootstrap de auth antes do merge. *Aceite:* testes existentes verdes; testes de fronteira reais (§4.3) verdes; `tests/team-contract` passam pelo adaptador; nenhuma rota para ações GLaDOS; zero edição em `team_*`/`owner`/`hub_auth`.

**F2 — Ownership de daemons, launchd, handoff (3–4 d + janela autorizada).** Fatos de identidade, `setsid`, `AbandonProcessGroup`, adoção por identidade+protocolo, remoção do `kill -9` por porta, estado persistido do watchdog, permits com outcome discriminado (§7). *Aceite (fixture inerte real, não strings):* SIGTERM/`kill -9`/`kickstart -k` do servidor ⇒ stubs vivos e adotados sem sinal; pid desconhecido na porta ⇒ não morto e reportado; `Gone` ⇒ 1 respawn; re-kick não dispara antes de grace+estado; lock: segundo controller recusa ações e nomeia o detentor. *Aceite operacional (janela):* ensaio de handoff (§7) com preimages; 1 restart de hub/app-servers documentado; rollback ensaiado.

**F3 — Witness de ciclo completo (3–4 d).** L2 (stubs, HOME sintético, `bd` real em fixture, hub isolado, Playwright): criar → aprovar (MCP GLaDOS) → bootstrap **2 seats Codex** → READY observado → mensagens → **falha induzida em duas formas distintas**: (a) erro de gate pré-exec ⇒ `gate_error(stage)`; (b) CLI que sai **após** `exec_boundary` sem observação ⇒ `root_exited_without_observation` — e o plano **não** as confunde → recovery explícita → parar → archive → readback; **restart da UI e do servidor no meio do ciclo** sem perda de estado. Depois, o mesmo roteiro **real** (harness reais) sob autorização explícita — sem simular PASS; NOT_RUN até rodar. `ui-deploy` sem restart. *Aceite:* receipt com SHAs de logs; o ciclo real registrado como PASS/FAIL/NOT_RUN honestamente.

**F4 — Inspeção de quarentena + sessão por time (3–4 d).** Plano PARKED com C1–C5; `window_id/pane_id` persistidos na criação (fato no-replace em `managed/gN`, pen Peppy pós-freeze); verbo `team_inspect_seat` **read-only** (operador); `apt-<team>`; T1–T7 + testes de retenção×cleanup (C5) sem start/ressuscitar. *Aceite:* herméticos + witness no L2; visão "Ended" só com binding persistido, senão "indisponível".

**F5 — Retirada do Tauri (1 d).** Só após F3 (real) e F4 aceitos: remover builder/bundle/installer/deps; app antigo retido como preimage; fallback passa a ser "reinstalar preimage em janela", nunca dois controllers. *Aceite:* build sem `@tauri-apps`; `just status` sem referência ao bundle; docs atualizados.

**Total:** 14–20 dias-agente (F0 root). Fora de escopo: LAN/remoto, terminal no browser, supervisor de CLI, migração de janelas incumbentes, presets UI, E2E amplo, política de merge de produto.

---

## 7. Coexistência, handoff e rollback (um controller, sempre)

- **Hoje não há lock** e o `RunEvent::Exit` do app mata hub/app-servers; o app na subida mata o squatter da porta. Logo "parar serviço e abrir app" **não** é inversa segura enquanto o app instalado for o atual. Regra: **coexistência = procedimento**, até que o app de fallback seja um build do mesmo head com lock (F1) — e mesmo então, fallback só com o serviço parado.
- **Handoff app → servidor (F2, janela autorizada, coordenação em pausa):** (1) preimages do app, bins, mcp dist, `~/.aperture/run/daemons/*`; (2) `Quit` do app (mata hub/app-servers — esperado, único restart); (3) instalar release `current` (bins cópia, `RELEASE.json`); (4) `launchctl bootstrap` do serviço: obtém lock, não encontra fatos ⇒ gera hub/app-servers **com fatos**; (5) verificar: hello real, presença dos agentes de coordenação, `team_list` byte-igual ao snapshot pré-quit, panes de coordenação reabertos se necessário; (6) só então browser.
- **Rollback (janela):** (1) `launchctl bootout` do serviço (daemons sobrevivem por `setsid`); (2) `aperture-server daemons stop` **explícito** (mata só pids dos fatos, por identidade; nunca por porta) — ou, se o app de fallback já for lock-aware, deixar que adote; (3) restaurar preimages **como conjunto compatível** (app + bins + dist); (4) abrir o app; (5) não restaurar owner/revocation mutáveis; efeitos não resolvidos ficam UNKNOWN (safety-contract §184-187).
- **Comandos em voo num restart do servidor** — outcome discriminado, sem retry automático: leituras: repetir; `create/approve/archive`: journalados pelo engine, readback decide; `prepare_replacement` **tem efeitos nativos** (stop/verify da incarnação antiga, safety-contract §23-46) ⇒ permit perdido **não é gratuito**: `start` responde `E_REPLACEMENT_PERMIT_LOST` com readback do owner (estado pós-prepare visível na UI) e o operador decide um novo `prepare` (semântica de reprepare existente) — nunca `start` inferido; `bootstrap_seat`: outcome pelo owner/attempt em disco (fatos de `abeb9ae`).

---

## 8. Riscos

| Risco | Mitigação |
|---|---|
| Dois controllers (app atual + serviço) | Procedimento F2; lock a partir de F1; app atual nunca aberto com serviço vivo |
| Adoção falsa (pid reciclado, socket de outro processo) | identidade `(pid, start_time)` + prova de protocolo (`presence.json.hub_pid`, `LOCAL_PEERPID`); nunca kill por porta |
| Restart do servidor perde permits | outcome discriminado + readback; sem retry |
| Release pareada: servidor de outro checkout quebra pins | pins passam a apontar para `releases/<sha>`; helpers copiados da mesma release; retenção enquanto referenciados |
| Token vaza | troca de uso único, 30 s, fragmento apagado, sessão em memória, rotação por subida; revisão Cipher |
| F4 muda `pane_dead`/cleanup | testes C5 antes de qualquer live |
| Confundir diagnóstico com causa | §6 F3 separa (a)/(b); Fable permanece não provado até o ciclo real |
| Coordenação Codex disrompida no handoff | 1 restart declarado, janela com coordenação em pausa, Open para reabrir panes |

---

## 9. Decisões genuínas ainda abertas (com default)

1. **Janela de handoff:** aceitar **um** restart de hub/app-servers com GLaDOS/Peppy em pausa (default: sim, agendado) — ou exigir adoção zero-restart, que não é possível a partir do app atual (limite declarado).
2. **Revisão de segurança do bootstrap de auth:** Cipher revisa §4.3 antes da F1 (default: sim; bloqueante).
3. **Retenção de releases antigas:** manter até archive dos seats que as referenciam (default) vs. limite de N releases com bloqueio se referenciadas.

Tudo o mais que estava nas 8 perguntas anteriores está decidido (browser-only, local-only, Tauri retirado, F4 obrigatória, readiness obrigatória, porta = detalhe técnico default `4519`, perda de estado tratada em §7).

---

## 10. Composição futura (time 2×Codex — ainda não criado)
Implementador: F1a `lib.rs`+bin+middleware; F1b handlers; F1c adaptador TS/justfile; F2 daemons/launchd; F3 witness; F4 (pens pós-freeze em `team_claude_launch.rs` coordenadas com Peppy). Revisor: por SHA, critérios acima, verify-against-reality (pids, hashes, `list-panes -a`, `lsof`). Root: F0, janelas, autorização do ciclo real e da F5. Cipher: §4.3. Izzy: QA do witness. **Não fazer:** rota genérica; bearer para ações GLaDOS; kill por porta; tocar `team_*`/`owner`/`hub_auth` fora do mapa; instalar sem preimages; simular PASS.

---

## Apêndice A — inventário Rust (@5394ae0; `abeb9ae` só altera `team_claude_launch.rs` e `team_replacement_native.rs`)
64 arquivos / 42.634 linhas. **NONE (58):** todos `team_*` (archive/auth/checkpoint/claude_*/launch_*/model/process/remote/replacement/repository/revoke/runtime), `owner.rs`, `journal.rs`, `hub_auth.rs`, `launcher.rs`, `agent_loader.rs`, `config.rs`, `state.rs`, `watchdog.rs`, `poller.rs`, `ws_hub.rs`, `codex_appserver.rs`, `bin/*`, 23 arquivos só de teste. **COMMAND-ONLY (4):** `agents.rs` (6 registrados), `teams.rs` (10 registrados; helpers privados `engine_from_state :1879`, `permit_store :2078`), `tmux.rs` (2 registrados: `tmux_create_session`, `tmux_select_window`; 4 anotados **não** registrados: `list_windows/create_window/kill_window/send_keys`), `team_terminal.rs` (1; `spawn_blocking :677`). **DEEP (2):** `lib.rs` (`:165,:243-245,:275,:282-284`), `main.rs`. **Registrados: 21** (agents 6 + tmux 2 + teams 10 + terminal 1 + `get_version` 1). Headless: `managed_claude_gate/observe`, `attach_managed_terminal`, `boot_agent_headless`, `team_control_json`; bins e seus contratos em §2.

## Apêndice B — frontend/testes (@5394ae0)
2.125 linhas TS. `invoke` em 4 services (14 verbos: `tauri-commands.ts` 9 sem injeção; `team-commands` 2, `team-runtime` 4, `team-terminal` 1 com `create*Commands(call)`); componentes que importam `commands` direto: `AgentList`, `AgentCard`, `AgentConfigModal`, `Footer`, `main.ts`. Zero `listen()`/plugins/WebSocket/`fetch`. `vite.config.ts` 1420/1421; `tauri.conf.json` `csp:null`. Testes: 10 `.mjs` (~121 casos), Vite middleware + `ssrLoadModule`, DOM fake, sem browser/Tauri; `real-bd-team-seat` usa `bd` real; sem `pnpm test`.

## Apêndice C — instalação/árvore (hoje)
`pnpm tauri build` v3.2.0; `.command` no Desktop via `publish-desktop-installer.py`; nenhum target `just` instala bundle/helpers; comando de helpers só em docs. Resolvidos por `CARGO_MANIFEST_DIR`: `mcp-server/dist/{index,ws-hub,hub-client}.js`, `mcp-server-sentry/dist/index.js`, `target/release/aperture-boot` (Open Codex). Helpers `~/.aperture/bin/*` separados; `aperture-boot` pinado por hash por lançamento (`team_claude_launch.rs:608-617,1245-1284`); `aperture-team-control` verificado por modo/uid/nlink sem hash (`team-control.ts:128-146`). Estado fora do bundle: `~/.aperture/{run/**,teams/**,repositories.json,agent-config.json,messages.db,mailbox,.beads,send-queue,objectives.json,bin}`, `~/.claude/aperture/**`. Sessão tmux `aperture` hardcoded (`src/main.ts:11,26`; `config.rs:55`; `team_claude_launch.rs:1377-1383`; `team_terminal.rs:797`; `team_claude_kickoff.rs:114`).

## Apêndice D — contrato de transporte (rascunho para revisão de segurança)
Bind `127.0.0.1:4519`. `Host` ∈ {`127.0.0.1:4519`,`localhost:4519`}; `Origin` obrigatório em mutantes; `Sec-Fetch-Site` ≠ `same-origin` ⇒ 403. Token operador 32 B, 0600, rotação por subida; troca de uso único (30 s) → sessão em memória; sem cookie; sem WS. **Rotas do operador (19):** `GET /api/version`; `GET /api/agents`; `POST /api/agents/{name}/{start|stop|restart}`; `POST /api/agents/{name}/model`; `POST /api/agents/{name}/attention/clear`; `POST /api/tmux/session`; `POST /api/tmux/select-window`; `GET /api/teams/catalog`; `GET /api/teams/presets`; `POST /api/teams/presets`; `POST /api/teams`; `GET /api/teams`; `POST /api/teams/{team}/cancel`; `POST /api/teams/{team}/seats/{seat}/bootstrap`; `POST /api/teams/{team}/replacement/prepare`; `POST /api/teams/{team}/replacement/start`; `POST /api/teams/{team}/archive`; `POST /api/teams/{team}/seats/{seat}/open`; (F4) `GET /api/teams/{team}/seats/{seat}/inspect`. **Nenhuma** rota para Approve/StopSeat/RetireSeat/ValidateCheckpoint/Replace/RollbackArchive/probes (GLaDOS via MCP). Erros `TeamError{code,message}`; novo código `E_REPLACEMENT_PERMIT_LOST`. Static de `releases/current/ui`; `index.html` `no-cache`; CSP `default-src 'self'`.

## Apêndice E — respostas ao ROOT REVIEW (35crof), item a item
1. **F4/READY/ciclo obrigatórios:** §0 "resultado aceito", §5, F3/F4 em §6. Política de merge de produto declarada fora (§6 "Fora de escopo").
2. **Restart/adoção:** §4.1 substitui "adotar se saudável" por identidade `(pid, start_time)` + prova de protocolo + fato de ownership no-replace + `setsid`/`AbandonProcessGroup`; kill por porta **removido**; `codex_appserver:95` reconhecido como connect-only e substituído por adoção por `LOCAL_PEERPID`; watchdog com estado persistido; limites declarados (sem `waitpid`; hub do app atual não adotável; um restart no handoff). Prova por fixture inerte real em F2.
3. **Rollback/handoff:** §7 — lock de controller, handoff exclusivo com preimages e conjunto compatível, coexistência = procedimento até o fallback ser lock-aware, outcome discriminado para comandos em voo e `prepare` com efeitos (`E_REPLACEMENT_PERMIT_LOST` + readback, sem retry).
4. **Fonte de verdade:** F0 nomeia `abeb9ae` como head, branch de implementação sobre ele, auditoria de branches por **conteúdo** com a lista concreta (§6 F0); inventário concreto em §4.2/Apêndice A com contagem correta (21 registrados, 4 anotados não registrados, 19 rotas).
5. **Transporte:** §4.3 matriz ator/ação — `operator_ui` ≠ GLaDOS, 19 rotas só do operador, ações GLaDOS ficam no MCP/bin sem rota; WS omitido; bootstrap de auth como decisão técnica com revisão Cipher (troca de uso único, sem token durável em URL/argv/log/referrer); testes de fronteira reais; validação por strings excluída.
6. **Atualização:** §4.4 fronteira de release `releases/<sha>` com `infra` do seam existente (desliga `CARGO_MANIFEST_DIR` do checkout vivo), retenção enquanto referenciada, helpers copiados e substituídos só em janela, contrato de compatibilidade UI↔API↔fatos; sem broker/framework.
7. **Perguntas:** §9 reduzidas a 3 genuínas com default; browser/local/F4/readiness/porta/perda de estado tratados como decididos.
8. **Causalidade:** §6 F3 separa (a) erro de gate pré-exec de (b) CLI que sai após `exec_boundary`; ciclo com 2 Codex reais, falha/recovery/stop/archive e restart de UI/servidor; real só autorizado; nada simulado como PASS; Fable não declarado resolvido.
Decisão do operador (7zmeyk) incorporada: §0, §4.2 REMOVER, F5, §9 (não é mais pergunta).

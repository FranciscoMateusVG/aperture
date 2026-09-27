# Aperture Web Local — documento de decisão

**Bead:** aperture-v3eqx · **Autor:** Wheatley · **Status:** rascunho para revisão de root (GLaDOS) e aprovação do operador. Nenhuma implementação, migração, instalação ou operação live está autorizada por este documento.

**Pins reais (verificados 2026-09-27):** `origin/master` = `2703dc4` (PR #71) · instalada = `5394ae0` · root = `d6d01cc` · freeze diagnóstico Peppy = `abeb9ae` (revisão independente PASS, note em k7y2s). **O runtime V4 (~48k linhas `team_*`) não está em `origin/master`**: vive nas branches `k310b`/`pm2rv`/`odhhn`/`k7y2s-launch-diagnostics`. Toda referência de linha abaixo é `@5394ae0` salvo indicação.

---

## 0. Resumo executivo

- **Problema** (operador, 27/set): confiabilidade dos times e ciclo de atualização/teste da interface — não entregar PRs de produto. Hoje cada mudança de UI exige rebuild + reinstalação do app Tauri; os daemons de runtime (hub, app-servers Codex, watchdog, poller) vivem **dentro** do processo da GUI e morrem com ela; a autoridade do operador é o próprio canal IPC do Tauri.
- **Recomendação:** um **servidor local em Rust (`aperture-server`)** que reutiliza a lib existente `aperture_lib` sem reescrita — 58 dos 64 arquivos Rust já são headless — e assume os 4 daemons com ciclo de vida próprio; a **UI atual servida no navegador** com um adaptador de transporte de 14 verbos; **workers/tmux inalterados** (já sobrevivem à GUI); a casca Tauri fica opcional e coexistente até a fase final. Loopback só, token de operador local, validação de Origin/Host, nenhuma rota de shell arbitrária.
- **O que NÃO fazer:** reescrever por linguagem (Rust→TS ou vice-versa); transportar a complexidade do plano de maio/2026 (Mac Mini, Tailscale, `0.0.0.0`, CORS permissivo, xterm/warroom inexistentes); criar broker/scheduler/novo canal; capturar stderr bruto de pane; prometer que isto resolve a causa do QA Fable.
- **Custo estimado:** 10–15 dias-agente (time 2×Codex, sequencial após integrar o freeze de Peppy), 4 fases com critérios mensuráveis; rollback = parar o serviço e abrir o app atual (mesmos binários auxiliares, mesmo estado em disco).

---

## 1. Prior art (citado, não repetido)

| Fonte | O que já resolve | Como este plano a trata |
|---|---|---|
| `docs/MIGRATION-WEB-APERTURE.md` (`7edfc2b`, 2026-05-09, em master) | "Manter Rust, trocar IPC Tauri por Axum HTTP/WS; conversão mecânica de `#[tauri::command]`" | **Reaproveitado na tese central.** **Supersedido** em: alvo (Mac local, não Mini), exposição (`127.0.0.1`, não `0.0.0.0`/Tailscale), CORS (nenhum), "abandonar Tauri" (coexistência primeiro), módulos citados que não existem mais (`warroom/spawner/xterm/Terminal.ts`), polling `capture-pane` para terminal (não haverá terminal simulado). |
| `docs/runtime-v4-safety-contract.md` §131-198 "Installation / coexistence / rollback (not executed)" | Freeze cumulativo, fixtures isolados, preimages, helpers pareados, rollback ≠ apagar journal | **Adotado integralmente** como procedimento de instalação/rollback da Fase 2 (ver §7). |
| `docs/runtime/*.md` (11 contratos) | Bootstrap/recovery Claude, gate, inbox probe, modelos, retirement, terminal, registry, MCP readiness, installer | Contratos **mantidos**; só `managed-terminal.md` e `installer-publication.md` mudam (Apêndice A). |
| Epic 4rsnc — retrospectiva 1º time | TOP: P1 readiness hello, P2 conclusão≠PR-open, P3 attach managed, P4 smoke Claude, P5 project=repo. **Não construir:** scheduler/broker, polling global, presets UI, novo canal, E2E browser amplo, novo harness Claude | Respeitado: nenhum canal novo (o hub existente continua o único bus), nenhum E2E amplo (um witness de jornada), nenhum harness. |
| k7y2s | Gate perde stderr no pane (vfrdnb); diagnóstico durável de Peppy (`abeb9ae`, PASS); plano inspeção/tmux-por-time **PARKED** com correções C1–C5 | Diagnóstico entra como **REUSAR** (core standalone); inspeção/tmux-por-time entra como **Fase 4 condicionada** às correções C1–C5. |
| `tests/boot-harness` (xt16e) | L1 cargo / L2 stubs CLI + hub isolado + sessão tmux efêmera / L3 real | Base dos "testes sintéticos isolados" e do "witness real explícito" (§6). |
| `docs/superpowers/specs/2026-09-06-aperture-v4-project-teams-design.md` v2.7 | Vocabulário, layout de runtime, fases P0–P4 | Nenhuma menção a servidor local/web, testes browser ou atualização sem reinstalar — lacuna que este documento preenche, sem alterar o layout de runtime. |

---

## 2. Estado atual — fatos com fonte

**Processos e ciclos de vida hoje.** O app Tauri é simultaneamente GUI e supervisor: `lib.rs::run()` (`:166-284`) repara PATH, inicializa dolt/bd, provisiona o token do watchdog, e sobe quatro daemons **no mesmo processo**: `poller` (thread 5s, badges em memória; `lib.rs:220`/`poller.rs:47`), `ws_hub` (thread + filho `node mcp-server/dist/ws-hub.js` em `127.0.0.1:4517`, respawn 2s, **mata com -9 qualquer processo que ocupe a porta**; `ws_hub.rs:84-126`), `codex_appserver` (1 thread + filho `codex app-server --listen unix://~/.aperture/run/<seat>.sock` por agente Codex; `codex_appserver.rs:86-141`; caminho "reusar socket vivo" em `:95`), `watchdog` (3 threads: subscriber WS ao hub, decision loop 2s com re-kick, varredura `bd` 45s; `watchdog.rs:286-315`). Único hook de encerramento: `tauri::RunEvent::Exit` (`lib.rs:282-284`). **Sobrevivem à GUI:** filhos managed do launch gate (`setsid`, `team_launch_gate.rs:79-104`, rastreados por owner records) e todos os panes tmux (servidor tmux externo). **Morrem com a GUI:** os 4 daemons e o estado em memória (badges, presença, `AppState.team_preparations`).

**Acoplamento Tauri (inventário completo, Apêndice A).** 64 arquivos Rust / 42.634 linhas: **58 sem nenhum uso de Tauri**, 4 só com wrappers `#[tauri::command]` (`agents.rs` 6, `teams.rs` 10 + 2 helpers privados tipados em `tauri::State`, `tmux.rs` 2 registrados, `team_terminal.rs` 1), 2 "profundos" (`lib.rs` builder/handler/`generate_context`/`RunEvent`; `main.rs` só chama `run()`). Zero uso de `AppHandle`/`Manager`/`Emitter`/eventos. **Superfície headless já existente:** `managed_claude_gate`, `managed_claude_observe`, `attach_managed_terminal`, `boot_agent_headless`, `team_control_json` (`lib.rs`), bins `aperture-boot` e `aperture-team-control` (stdin JSON → stdout JSON). Consequência: "manter Rust" custa ~40 linhas de casca; o trabalho real está em (i) mover daemons para um processo próprio, (ii) substituir a autoridade implícita do IPC, (iii) manter o permit store num processo de vida longa.

**Autoridade do operador hoje = o canal IPC.** Os comandos cunham `AuthenticatedActor::operator_ui()` incondicionalmente (`teams.rs:1897,1902,1912,2093`). Um servidor web precisa de credencial de operador explícita e verificável — ponto central do contrato de transporte (§4.3, Apêndice D).

**Frontend (Apêndice B).** 2.1k linhas TypeScript DOM (sem React). Único ponto de contato com Tauri: `invoke` em **4 services** (14 verbos), 3 dos quais já recebem `call` por injeção (`createXCommands(call)`); `tauri-commands.ts` (9 verbos) não é injetável. Zero `listen()`, zero plugins, zero WebSocket ao hub — presença chega por polling 3s em `list_agents`. Testes: 10 arquivos `node:test` (~121 casos) que sobem Vite em middleware e carregam os módulos reais com DOM fake — **rodam sem Tauri e sem browser**; não há `pnpm test` nem receita `just` para eles.

**Instalação/atualização hoje (Apêndice C).** Bundle Tauri (`pnpm tauri build`, versão 3.2.0) instalado por um `.command` publicado no Desktop (`scripts/publish-desktop-installer.py`). Vários artefatos "instalados" são resolvidos por `CARGO_MANIFEST_DIR` fixado em compilação (o checkout de onde o app foi construído): `mcp-server/dist/{index,ws-hub,hub-client}.js`, `mcp-server-sentry/dist/index.js`, `target/release/aperture-boot` para Open Codex. Helpers em `~/.aperture/bin/{aperture-boot,aperture-team-control}` são arquivos separados, **não copiados por nenhum código** (procedimento manual, safety-contract §144-151); `aperture-boot` é pinado por hash **por lançamento** no `LaunchRecord`. Todo o estado de runtime vive fora do bundle (`~/.aperture/{run,teams,repositories.json,agent-config.json,messages.db,mailbox,.beads,bin}`, `~/.claude/aperture/*`) e sobrevive a reinstalação. Sessão tmux `aperture` é hardcoded na GUI (`src/main.ts:11,26`; `config.rs:55`) e nos lançamentos Claude (`team_claude_launch.rs:1377-1383`); `APERTURE_TMUX_SESSION` só vale para `aperture-boot`/headless.

---

## 3. Alternativas (comparação pequena)

Critérios: (a) independência de ciclos UI ↔ servidor ↔ workers; (b) reuso das correções revisadas (k310b/pm2rv/odhhn/k7y2s); (c) contrato local auth/origem sem shell; (d) testabilidade sintética; (e) reversibilidade; (f) custo.

| | A. Manter Tauri, só corrigir o ciclo de UI | **B. `aperture-server` Rust (Axum) sobre `aperture_lib`** | C. Servidor Node chamando bins headless | D. Node UI + daemon Rust por socket |
|---|---|---|---|---|
| (a) | ✗ daemons continuam na GUI; refresh/reinstalar mata hub/watchdog | ✔ daemons no serviço; browser descartável; workers já independentes | ◐ precisa de um daemon Rust mesmo assim (watchdog/poller/app-servers estão em Rust) → vira D | ✔ |
| (b) | ✔ | ✔ 58/64 arquivos intocados; wrappers mecânicos | ✗ só `team_control_headless` está coberto pelo bin; `prepare/start_replacement`, `agents start/stop/list`, badges não têm entrada headless | ◐ |
| (c) | n/a (IPC) | ✔ token local + Origin/Host + rotas finitas | ◐ dois processos, duas superfícies de auth | ✗ novo canal socket + contrato — vetado pela retrospectiva 4rsnc |
| (d) | ✗ sem browser real | ✔ handlers Axum com `AppState` fake; browser contra HOME sintético | ◐ | ◐ |
| (e) | ✔ | ✔ coexistência com o app até a última fase; rollback = parar serviço | ◐ | ✗ |
| (f) | baixo, não resolve o problema | **médio** (10–15 d-agente) | médio-alto (reimplementar daemons ou cair em D) | alto |

**Escolha: B.** É a tese do prior art de maio, corrigida para o alvo real (local, loopback, coexistência) e aplicada a um crate que já é uma lib headless.

---

## 4. Recomendação e arquitetura alvo

### 4.1 Processos e ciclos de vida
```
browser (descartável)  ──HTTP/WS loopback+token──▶  aperture-server (launchd, reiniciável)
                                                     ├─ serve ~/.aperture/ui/current (dist)
                                                     ├─ API = os 21 comandos atuais, 1:1
                                                     ├─ hub supervisor (ADOTA hub saudável; não mata)
                                                     ├─ codex app-server supervisors (adotam sockets vivos)
                                                     ├─ watchdog + poller (estado reconstruído do hub/FS)
                                                     └─ AppState (permit store) em processo de vida longa
tmux "aperture" (coordenação) + sessões por time ◀── workers: filhos setsid + panes — INDEPENDENTES de tudo acima
```
- **Refresh/fechar o browser:** nada acontece no runtime (a UI já é stateless: `list_agents`/`team_list` reconstroem tudo do filesystem + tmux + hub a cada poll).
- **Reiniciar `aperture-server`:** hub e app-servers **não** são mortos; na subida o supervisor **adota** o hub existente se responder ao hello (hoje `ws_hub.rs` mata o "squatter" — esta política muda: matar só se não responder), e reusa sockets Codex vivos (caminho já existente em `codex_appserver.rs:95`). Watchdog re-subscreve; presença é replay do hub. Único estado perdido: `team_preparations` (prepare→start em voo) — hoje já se perde em crash da GUI; fica documentado e o `start` responde erro claro.
- **Reconciliar ao reabrir:** fonte de verdade permanece o filesystem (`owner/`, `teams/`, `managed/gN` com os fatos diagnósticos de Peppy) + `tmux list-panes -a` + presença do hub. Nunca memória do servidor.

### 4.2 Mapa REUSAR / ADAPTAR / REMOVER
**REUSAR como está:** todos os `team_*` (58 arquivos NONE), `owner/journal/hub_auth/launcher/agent_loader`, bins `aperture-boot`/`aperture-team-control`, `mcp-server` (incl. `ws-hub.ts`), freeze diagnóstico `abeb9ae` (contexto antes da release, 7 steps no-replace, observation-first — core standalone: filesystem + bins, sem Tauri), `tests/boot-harness`, `tests/*.mjs`, `docs/runtime/*`, safety-contract §rollback.
**ADAPTAR (mínimo):** `lib.rs` (extrair `daemons::start()` de `run()`; novo bin `aperture-server`), `ws_hub.rs` (adotar-se-saudável em vez de matar; shutdown não mata em restart), `codex_appserver.rs` (adoção explícita na subida), `agents.rs/teams.rs/tmux.rs/team_terminal.rs` (wrappers → handlers com `Arc<Mutex<AppState>>`; `operator_ui()` cunhado **só** após middleware de auth), `src/services/tauri-commands.ts` (tornar injetável) + 4 services (adaptador de transporte), `src/main.ts` (bootstrap da sessão tmux passa ao servidor), `vite.config.ts` (proxy dev), `justfile` (`server-build`, `ui-deploy`, `test-ui`), `docs/runtime/managed-terminal.md` e `installer-publication.md`.
**REMOVER (só na Fase 3, reversível até lá):** builder/bundle Tauri, `tauri.conf.json`, `publish-desktop-installer.py`, dependências `@tauri-apps/*`.
**NÃO transportar:** warroom/spawner/xterm/CORS/`0.0.0.0` do plano de maio; scheduler/broker; novo canal; supervisor de CLI Claude; captura de stderr bruto; `select-pane -d` como "read-only"; `switch-client` em todos os clientes (C1–C2).

### 4.3 Contrato de transporte local (detalhe no Apêndice D)
Bind `127.0.0.1:<porta>` apenas; **Host** ∈ {`127.0.0.1:<porta>`, `localhost:<porta>`} e **Origin** obrigatório e igual em toda requisição mutante e no upgrade WS (defesa contra DNS rebinding e páginas de terceiros); **token de operador** de 32 bytes em `~/.aperture/run/operator.token` (0600, `O_NOFOLLOW`, mesma disciplina de `hub-tokens`), enviado em `Authorization: Bearer` — nunca cookie (sem auth ambiente ⇒ sem CSRF); WS autentica no primeiro frame como o hub já faz; comparação em tempo constante; rotação a cada subida do servidor; o browser obtém o token por URL impressa/aberta pelo próprio servidor (`aperture-server open`) com o token no fragmento. **Rotas = os 21 comandos atuais, 1:1**, mesmos DTOs de entrada/saída (os parsers/fixtures de `tests/team-contract` já os validam); **nenhuma rota genérica** de exec/tmux/send-keys; verbos tmux expostos continuam `create_session` e `select_window`. Não exposto à LAN por padrão; servidor remoto não é pedido.

### 4.4 Atualizações sem reinstalação
- **UI:** `just ui-deploy` = `pnpm build` → `~/.aperture/ui/<sha>/` + troca atômica do symlink `~/.aperture/ui/current` → recarregar a aba. Sem restart do servidor, sem app.
- **Servidor:** `cargo build --release --bin aperture-server` + `launchctl kickstart -k` (restart adota hub/app-servers/tmux; §4.1). Helpers `aperture-boot`/`aperture-team-control` seguem o procedimento pareado do safety-contract (mesmo checkout, mesmo `CARGO_MANIFEST_DIR`, hashes registrados) — restrição real, não removida por este plano.
- **MCP:** `just build-mcp` como hoje (o hub adotado só reinicia quando o operador decidir).

---

## 5. Requisitos do usuário → como o desenho atende

| Requisito | Atendimento | Evidência exigida |
|---|---|---|
| Times confiáveis ciclo completo | Runtime intocado + diagnósticos duráveis + daemons que não morrem com a UI | Jornada L2 (§6) criar→aprovar→bootstrap→mensagem→falha→recovery→parar→archive |
| Erro durável / falha cedo | Freeze `abeb9ae` REUSADO: `gate_error(stage, code)`, `root_exited_without_observation`, `observation_timeout` em `managed/gN` | Já provado (66/39/25 + minha reprodução 29/29, 64/64) |
| Inspeção de quarentena | Fase 4, **condicionada** às correções C1–C5: read-only por cliente (não `select-pane -d`), cliente do operador identificado (não `switch-client` em todos), binding histórico com `window_id/pane_id` persistidos (não nome+PID), "Quarantined sem pane" só após cleanup verificado | Testes herméticos + witness L2 |
| Sessão tmux por time, coordenação separada | Fase 4: `apt-<team>` + coordenação em `aperture`; incumbentes sem relaunch/move/kill; dependência de 2 linhas em `team_claude_launch.rs:1377-1383` | T1–T7 do plano parked |
| Testes browser + sintéticos isolados | `pnpm test` (node:test existente) + testes de handler Axum + **um** witness browser contra HOME sintético/stubs L2 | §6 |
| Atualização sem reinstalar por UI | §4.4 | `ui-deploy` sem restart, verificado por hash da `index.html` servida |
| Migração reversível sem derrubar runtime | Coexistência app↔servidor com lock de daemons; rollback = parar serviço + abrir app; estado em disco inalterado | §7 |

---

## 6. Fases, critérios de aceite mensuráveis, custo

Pré-condição: freeze `abeb9ae` integrado no head cumulativo (root). Time futuro: 2×Codex (implementador + revisor), pen disjunta por fase, revisão independente por SHA; **não criado ainda**.

**F0 — Congelar e preparar (root, 0,5 d).** Head cumulativo (k310b+pm2rv+odhhn+k7y2s) congelado e testado; `pnpm test` e `just test-ui` adicionados (só empacotam os testes existentes). *Aceite:* `cargo test --lib` e `node --test tests/*.mjs` verdes no head, hashes registrados.

**F1 — `aperture-server` + adaptador (3–5 d-agente).** Novo bin no mesmo crate; `daemons::start()` extraído de `run()`; middleware de auth/Origin/Host; 21 rotas 1:1; static `ui/current`; `tauri-commands.ts` injetável + adaptador HTTP/WS; app Tauri **continua funcionando** (build e comportamento byte-iguais nos módulos NONE). *Aceite:* (i) todos os testes existentes verdes; (ii) testes de handler: cada rota sem token ⇒ 401, Origin errado ⇒ 403, DTO inválido ⇒ mesmo `TeamError` de hoje; (iii) `tests/team-contract` fixtures passam pelo adaptador sem mudança; (iv) `strings`/config provam bind `127.0.0.1` e ausência de rota genérica; (v) nenhuma linha alterada em `team_*`, `owner.rs`, `hub_auth.rs`.

**F2 — Serviço, coexistência, adoção (2–3 d-agente).** launchd agent; lock `~/.aperture/run/server.lock`; app Tauri recusa subir daemons se o lock estiver vivo (e vice-versa); `ws_hub` adota hub saudável; app-servers adotados; procedimento de instalação/rollback do safety-contract executado **com autorização separada**. *Aceite:* (i) `kickstart -k` do servidor com 1 time ativo: nenhum pid de worker/app-server/hub muda, presença volta em ≤10 s; (ii) fechar/reabrir browser: `team_list` idêntico byte a byte; (iii) rollback: parar serviço → abrir app → mesmo `team_list`; (iv) coexistência: app + serviço simultâneos ⇒ o segundo recusa daemons com erro claro, nunca `-9` no hub.

**F3 — Witness browser + ui-deploy (2–3 d-agente).** Jornada L2 com HOME sintético, stubs `claude`/`codex` do boot-harness, sessão tmux efêmera e hub isolado, dirigida por browser real (Playwright ou o MCP playwright-mini já disponível): criar time de preset → aprovar → bootstrap → mensagem → **falha induzida** (stub sai antes do exec) ⇒ `gate_error(stage)` visível na UI → recovery explícita → parar → archive → readback. `just ui-deploy` + troca de symlink. *Aceite:* (i) jornada verde em L2 com receipt (SHAs de logs); (ii) L3 real registrado como NOT_RUN salvo autorização; (iii) `ui-deploy` sem restart: hash da `index.html` servida muda, pids do servidor/hub não.

**F4 — Inspeção de quarentena + tmux por time (3–4 d-agente; aprovação separada).** Plano PARKED de k7y2s com C1–C5 aplicadas; `window_id/pane_id` persistidos na criação da janela (fato novo, no-replace, em `managed/gN`); verbo `team_inspect_seat` read-only; `apt-<team>`; dependência de 2 linhas em `team_claude_launch.rs`. *Aceite:* T1–T7 + os testes de C5 (retenção × cleanup) sem start/ressuscitar.

**Total:** 10–15 dias-agente; 1–2 semanas de calendário com revisão. Fora de escopo: servidor remoto/LAN, terminal no browser, supervisor de CLI, migração de janelas incumbentes, presets UI, E2E amplo.

---

## 7. Riscos

| Risco | Mitigação |
|---|---|
| Dois supervisores do hub (app + serviço) ⇒ `-9` mútuo | Lock de daemons (F2) + política adotar-se-saudável antes de qualquer coexistência real |
| Token no fragmento de URL vaza em histórico/screenshot | Rotação por subida; `sessionStorage`; sem cookie; opção de `aperture-server open` que abre o browser sem imprimir |
| Perda de `team_preparations` em restart | Erro claro no `start`; documentado; já é o comportamento em crash |
| `CARGO_MANIFEST_DIR` pareado: servidor construído de outro checkout quebra pins de `aperture-boot`/MCP | Mesma regra do safety-contract; `just server-build` só do checkout canônico; receipt com hashes |
| Fase 4 depende de tmux `remain-on-exit`, que muda `pane_dead`/cleanup | C5: testes herméticos da interação antes de qualquer live |
| Achar que diagnósticos = causa Incluir resolvida | Explícito: `abeb9ae` diagnostica a **próxima** tentativa; a causa histórica permanece desconhecida |
| Escopo cresce para "terminal no browser" | Vetado aqui; Open/Inspect apontam o cliente tmux do operador, nunca simulam terminal |

---

## 8. Perguntas realmente humanas (operador)

1. Casca Tauri: manter como WebView fina apontando para o servidor (ícone/dock) durante F1–F3, ou browser-only desde F2?
2. Porta fixa do servidor (proposta `4519`, hub segue `4517`) e browser padrão para `aperture-server open`.
3. Aceita token no fragmento da URL de abertura (rotacionado por subida) como método de bootstrap do browser?
4. launchd `KeepAlive` com início no login: sim/não?
5. Aceita perder um `prepare→start` em voo num restart do servidor (erro claro, sem retry)?
6. Confirma que **não** quer exposição LAN/Tailscale (o plano de maio é formalmente arquivado)?
7. Fase 4 (inspeção/tmux por time) entra neste ciclo ou fica para depois de F3?
8. Dos TOPs da retrospectiva 4rsnc, quais entram junto: P1 readiness do hello, P2 política de merge por repo?

---

## 9. Composição futura (time 2×Codex — ainda não criado)

- **Lead/implementador:** F1–F3 em pens sequenciais e disjuntas: (F1a) `lib.rs`+`bin/aperture-server.rs`+middleware; (F1b) handlers em `agents/teams/tmux/team_terminal`; (F1c) `src/services/*` + `vite.config` + `justfile`; (F2) `ws_hub.rs`/`codex_appserver.rs`/launchd; (F3) witness + `ui-deploy`.
- **Revisor:** revisão independente por SHA a cada fase com os critérios de aceite acima; sem auto-aprovação; verify-against-reality (pids, hashes, `list-panes -a`).
- **Root (GLaDOS):** freeze cumulativo, autorização separada para instalação (F2) e para F4; QA de jornada com Izzy no witness L2.
- **Não fazer:** tocar `team_*`/`owner`/`hub_auth` fora do mapa ADAPTAR; abrir rota genérica; mudar constantes de orçamento; instalar sem preimages; migrar janelas incumbentes; ler segredos.

---

## Apêndice A — inventário Rust (coupling, @5394ae0)
- 64 arquivos / 42.634 linhas. **NONE (58):** todos `team_*` (archive/auth/checkpoint/claude_*/launch_*/model/process/remote/replacement/repository/revoke/runtime), `owner.rs`, `journal.rs`, `hub_auth.rs`, `launcher.rs`, `agent_loader.rs`, `config.rs`, `state.rs`, `watchdog.rs`, `poller.rs`, `ws_hub.rs`, `codex_appserver.rs`, `bin/*` e 23 arquivos só de teste. **COMMAND-ONLY (4):** `agents.rs` (`start/stop/restart/list_agents`, `clear_attention`, `update_agent_model`), `teams.rs` (`team_get_catalog`, `team_list_presets`, `team_save_preset`, `team_create`, `team_list`, `team_cancel_pending`, `team_bootstrap_seat`, `team_prepare_replacement`, `team_start_replacement`, `team_archive`; helpers privados `engine_from_state :1879`, `permit_store :2078`), `tmux.rs` (`tmux_create_session`, `tmux_select_window` registrados; 4 anotados de uso interno), `team_terminal.rs` (`team_open_seat`, `spawn_blocking :677`). **DEEP (2):** `lib.rs` (`Builder/manage/generate_handler :243-245`, `generate_context :275`, `RunEvent::Exit :282-284`), `main.rs`.
- Headless: `managed_claude_gate`, `managed_claude_observe` (stdin), `attach_managed_terminal`, `boot_agent_headless`, `team_control_json`; bins `aperture-boot` (`--agent…`, `--managed-claude-observe`, `--managed-claude-gate`, `--attach-managed`), `aperture-team-control` (stdin JSON ≤16 KiB → stdout; exit 0/1/2).
- Daemons no processo GUI e o que sobrevive: ver §2. Autoridade `operator_ui()` cunhada no wrapper: `teams.rs:1897,1902,1912,2093`. Estado de processo único: `AppState.team_preparations`.

## Apêndice B — inventário frontend/testes (@5394ae0)
- `src/`: 2.125 linhas TS. Tauri só via `invoke` em `tauri-commands.ts` (9 verbos, sem injeção), `team-commands.ts` (2), `team-runtime.ts` (4), `team-terminal.ts` (1) — 14 verbos. Componentes que importam `commands` diretamente: `AgentList`, `AgentCard`, `AgentConfigModal`, `Footer`, `main.ts`; `TeamsArea`/`TeamLifecycle` recebem `api` injetável. Sem `listen()`, plugins, `__TAURI__`, WebSocket, `fetch`, `location`.
- Config: `vite.config.ts` porta 1420 strictPort, HMR 1421 só com `TAURI_DEV_HOST`; `tauri.conf.json` `csp: null`, `devUrl localhost:1420`, `frontendDist ../dist`; `index.html` sem CSP.
- Testes: 10 `.mjs` (~121 casos) com Vite middleware + `ssrLoadModule` + DOM fake; `real-bd-team-seat` usa `bd` real; `boot-harness` precisa tmux + `aperture-boot`. Sem `pnpm test`/receita `just`. Evidência registrada 83/83 num subset de 5 arquivos.

## Apêndice C — instalação e árvore de runtime (hoje)
- Build/instalação: `pnpm tauri build` (v3.2.0); `.command` publicado por `scripts/publish-desktop-installer.py`; nenhum target `just` instala bundle/helpers; comando de helpers só em docs (safety-contract §148-150). `just setup` symlinka `~/.claude/aperture/<agent>/*` ao repo e pula dirs com marcador `TEAM`.
- Resolvidos por `CARGO_MANIFEST_DIR` (checkout de compilação): `mcp-server/dist/{index,ws-hub,hub-client}.js`, `mcp-server-sentry/dist/index.js`, `src-tauri/target/release/aperture-boot` (Open Codex). Helpers `~/.aperture/bin/*` separados, pinados por hash por lançamento (`team_claude_launch.rs:608-617,1245-1284`); `aperture-team-control` verificado por modo/uid/nlink, sem hash (`team-control.ts:128-146`).
- Estado fora do bundle (sobrevive a reinstalação): `~/.aperture/run/{owner,hub-tokens,revocations,team-locks,managed,team-journals,archive-manifests,terminals,presence.json,*.sock,*.thread-id,<seat>.gN.*.json}`, `~/.aperture/teams/**`, `repositories.json`, `agent-config.json`, `messages.db`, `mailbox/`, `.beads`, `send-queue`, `objectives.json`, `bin/`, `~/.claude/aperture/**`.
- tmux: sessão `aperture` hardcoded (`src/main.ts:11,26`; `config.rs:55`; `team_claude_launch.rs:1377-1383`; `team_terminal.rs:797`; `team_claude_kickoff.rs:114`); `APERTURE_TMUX_SESSION` só headless.

## Apêndice D — contrato de transporte local (rascunho para revisão)
- **Bind:** `127.0.0.1:4519` (proposta). Nunca `0.0.0.0`; sem TLS local (loopback); sem CORS (mesma origem).
- **Identidade do operador:** `~/.aperture/run/operator.token` — 32 bytes aleatórios hex, 0600, criado/rotacionado pelo servidor na subida com `O_NOFOLLOW|O_EXCL` e diretório privado, mesma disciplina de `hub_auth`. Apresentado em `Authorization: Bearer <token>` (HTTP) e no primeiro frame `{"hello":{"token":…}}` (WS). Comparação em tempo constante; sem cookie; sem query string exceto o fragmento da URL de abertura (nunca enviado ao servidor).
- **Origem:** `Host` deve ser `127.0.0.1:4519`/`localhost:4519`; `Origin` obrigatório em POST/PUT/DELETE e no upgrade WS, igual a `http://127.0.0.1:4519` ou `http://localhost:4519`; `Sec-Fetch-Site` ≠ `same-origin` ⇒ 403. CSP servida: `default-src 'self'; connect-src 'self' ws://127.0.0.1:4519 ws://localhost:4519`.
- **Rotas (1:1 com os 21 comandos; DTOs idênticos):** `GET /api/version`, `GET /api/agents`, `POST /api/agents/{name}/{start|stop|restart}`, `POST /api/agents/{name}/model`, `POST /api/agents/{name}/attention/clear`, `POST /api/tmux/session`, `POST /api/tmux/select-window`, `GET /api/teams/catalog`, `GET /api/teams/presets`, `POST /api/teams/presets`, `POST /api/teams`, `GET /api/teams`, `POST /api/teams/{team}/cancel`, `POST /api/teams/{team}/seats/{seat}/bootstrap`, `POST /api/teams/{team}/replacement/prepare`, `POST /api/teams/{team}/replacement/start`, `POST /api/teams/{team}/archive`, `POST /api/teams/{team}/seats/{seat}/open`. **Sem** rota de exec/shell/send-keys/capture. Erros: `TeamError {code,message}` como hoje.
- **Estático:** `GET /` e assets de `~/.aperture/ui/current`; `index.html` com `Cache-Control: no-cache`; assets com hash.
- **WS `/ws/events` (F3, opcional):** só push de "mudou algo" para acordar o poll; nenhum dado de terminal.
- **Coexistência:** `~/.aperture/run/server.lock` (flock) — quem não o obtém não sobe daemons e informa qual processo os detém.

# Aperture Web — ícone macOS

O painel continua web; este aplicativo AppKit pequeno só controla o servidor local
já empacotado. Não contém Tauri/WebView, não instala serviço/launchd nem roda agentes.

## Uso

- Abra **Aperture Web.app** (ou seu atalho na Mesa/Dock). Se o servidor estiver
  parado, inicia uma vez; se estiver ligado, reutiliza. Abre sessão autenticada no
  navegador pelo comando nativo `aperture-server open`.
- No aplicativo ou ícone da barra de menus, **Encerrar servidor…** fecha o painel
  usando o drain nativo. **Agentes e hub continuam rodando.** Parar esses agentes
  é outra ação, feita no painel antes de desligá-lo.
- Fechar uma aba não para o servidor. Fechar a janela do lançador mantém o ícone
  disponível. **Sair do lançador…** confirma que o servidor continuará ligado.
- Para manter no Dock: botão direito no novo ícone → Opções → Manter no Dock.
  O aplicativo Tauri antigo é preservado, mas não é o novo lançador.
- Após reiniciar o Mac, abra o novo ícone. Não há inicialização automática no login.
  Dependências locais do servidor continuam necessárias (incluindo tmux configurado).

## Build / entrega

```
just web-launcher-build '/absolute/new/Aperture Web.app' \
  '/absolute/package/bin/aperture-server' '/absolute/real/node'
just web-launcher-test
```

`build.py` usa apenas Python/Swift/AppKit nativos; destino exclusivo, assinatura ad-hoc
local. Não substitui app existente, instala ou executa nada. Entregue o pacote do
servidor primeiro. `Launcher.json` contém somente os dois caminhos absolutos, sem
segredos. Uma atualização de pacote exige atualizar/reentregar o lançador.

## Contrato do servidor

`aperture-server status` imprime `running`, `stopping` ou `stopped` e sai 0;
respostas estranhas, falta de autoridade ou conexão recusada com lease ocupada
são erros, não permissão para iniciar. `stop` envia **um** POST autenticado e
observa porta fechada **e** lease liberada antes de sucesso. Nunca envia sinal.
As rotas status/shutdown aceitam somente o capability nativo atual, rejeitam
Origin/Sec-Fetch e não aceitam a sessão do navegador. O token fica em memória,
fora de argv, ambiente, logs e resposta. `open` mantém o exchange efêmero já existente.

O lançador serializa abrir/encerrar (inclusive Dock reopen). A lease do servidor
resolve concorrência entre lançadores/processos. Não mata nada por PID/porta/nome,
não muda tokens de agentes/hub, não faz retry de shutdown ou force-stop. Um timeout
é estado **não confirmado**, nunca sucesso. O timeout de um CLI curto pode enviar
TERM somente ao próprio filho; nunca ao servidor. Falha de startup deixa o log em
`~/Library/Logs/Aperture/server-<UUID>.log` (0600, diretório 0700) e não reinicia.
Nenhum log antigo é apagado.

## Evidência e limites

Regressões TCP usam HOME sintético e o router real: autoridade nativa vs browser,
wrong service, parada, lease ainda ocupada e liberação. Testes Swift usam controle
injetado para reutilização/start único, concorrência, erro, stopping e stop único.
Readback de entrega registra separadamente o ciclo operacional e a preservação dos
PIDs/births dos agentes/hub. Reinício completo do Mac, login automático e provider
não são certificados pelo teste de abrir/encerrar servidor.

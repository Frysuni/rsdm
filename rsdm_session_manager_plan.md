# План корректного завершения сеансов RSDM

## Цель

Дать приложениям возможность завершиться, пока compositor и необходимые
сервисы доступны, затем завершить графический lifecycle и закрыть PAM.
Основа — согласованный план после исследования
`rsdm_session_manager_shutdown_architecture.md`; исходный документ является
материалом для обсуждения, а не спецификацией реализации.

## Согласованные ограничения

- Linux с systemd, основной минимум 250 с проверкой необходимых capabilities.
- Первоначальный охват — приложения, запущенные через `rsdm app`.
- Никаких правил, профилей и проверок имён конкретных приложений.
- Никаких новых таблиц WM/DE и shutdown-параметров в `rsdm.toml`.
- Небольшие встроенные адаптеры WM/DE, ручные уточнения через CLI.
- GNOME/Plasma сохраняют штатные протоколы сохранения, отмену и inhibitors.
- Таймаут приложения по умолчанию 30 секунд; `--on-timeout force` по умолчанию.
- `--on-timeout cancel` защищает выбранное приложение в управляемой фазе.
- Отмена сохраняет compositor и инфраструктуру; закрытые приложения не
  восстанавливаются и принятый приложением запрос выхода не отзывается.
- XSMP — отдельный обязательный этап после основного lifecycle.
- Один графический сеанс на UID: bus, environment и graphical targets общие.
- Root не исполняет пользовательские quit-команды и не повышает полномочия
  пользовательского запроса питания.
- Признаки падения в файлах приложений не изменяются.
- Ограничение батарейки снято: проверки и сборки разрешены.
- rustfmt/cargo fmt, Clippy/cargo clippy и actionlint не запускать и не
  добавлять в CI. AGENTS.md не коммитить и не добавлять в gitignore.
- Релиз, bump версии и push не являются частью текущей реализации.

## Подтверждённые исходные проблемы

- Текущий supervisor останавливает compositor раньше app teardown.
- `PartOf` распространяет stop, но не запрашивает сохранение приложения.
- В действующем сеансе niri запущен временным Type=exec сервисом RSDM,
  штатный niri.service неактивен. Официальный niri-session имеет особую
  ветку запуска непосредственно из systemd; её нельзя случайно выбирать.
- `rsdm app` проверяет target отдельно от создания unit: возможен запуск
  после начала выхода.
- Session.id — desktop entry ID, а не logind session ID.
- Wrapper не передаёт фактически выбранный config path и session metadata.
- finalize может активировать target во время завершения.
- На исследованной машине InhibitDelayMaxUSec=5000000; app timeout не
  увеличивает бюджет внешнего shutdown.
- Нет универсального org.freedesktop.Application.Quit. Wayland session
  management занимается восстановлением окон, а не общим quit/save.
- PAM принадлежит отдельному root leader, desktop переживает restart DM:
  эти свойства необходимо сохранить.

## Архитектура

### Координатор и идентичность

`rsdm session start` остаётся пользовательским координатором без нового
root daemon. Разделить orchestration/state, transport, app lifecycle,
адаптеры окружений, readiness/environment и XSMP по ответственности.
Вынести session/app CLI из main.rs, который уже содержит 313 строк.

Пользовательский endpoint org.rsdm.Session1: launch, finalize, stop, cancel,
status. Обработчики доставляют события одному координатору. Длительные
systemd операции не блокируют приём отмены и notifications.

Развести desktop_entry_id, проверенный logind login_session_id и уникальную
generation. Запуски и requests несут generation, старые requests отвергаются.
Для приложений фиксировать unit, InvocationID, policy, launch/stop state.
Стабильная часть app identity не изменяется, generation относится к instance.

Registry хранится в памяти. Минимальный recovery record — в XDG_RUNTIME_DIR
с directory 0700 и files 0600, без сохранения полного окружения и без вывода
его значений в журнал. Cleanup затрагивает только доказанно принадлежащие
generation units и не использует KillUser или широкие glob-паттерны.

### Systemd и readiness

Типизированный D-Bus через zbus 5.14, совместимый с Rust 1.88, без Tokio.
StartTransientUnit, job subscription до отправки request, ожидание точного
job, отслеживание InvocationID/processes, pidfd для проверенного main process.
ExecStartEx с no-env-expand сохраняет argv без shell и substitution.

Managed compositor: After graphical-session-pre.target, Before graphical
и autostart targets. Anchor service конкретной generation держит lifecycle.
Apps: After/Requisite anchor + graphical target, PartOf соответствующего
lifecycle. Autostart стартует после readiness. Не ставить anchor после
autostart target, чтобы не создавать цикл. Новый shutdown target не нужен.

Finalize проходит через coordinator, проверяет generation/state, публикует
env и readiness; после начала shutdown отвергается. Автоматическая readiness
остаётся запасной. Повторное имя WAYLAND_DISPLAY не означает старый сеанс.
Wrapper получает config path и выбранную desktop metadata.

### WM/DE

- Generic managed WM: RSDM закрывает managed apps перед compositor.
- Официальный niri-session запускается вне временного compositor service;
  наблюдать настоящий niri.service и его InvocationID/notify readiness.
- GNOME/Plasma: исходный Exec сохраняется, logout/power делегируются штатным
  managers. D-Bus method reply не означает завершение logout.
- DesktopNames — подсказка, владение подтверждается unit/native API.
- Native readiness требует готового manager и его активного graphical target;
  RSDM не активирует этот target вместо native manager.
- CLI overrides: --mode auto|managed|external, --native-unit UNIT,
  --logout-command "command arguments". Не заменять Exec догадками.
- Не добавлять второй wrapper, если desktop entry явно вызывает session
  start; второй coordinator одной generation отвергать.
- В native DE policies действуют только в RSDM-controlled phases.
  Неподдерживаемую комбинацию отклонять заранее, не обходить native save.

## CLI и последовательность завершения

Сохранить `rsdm app -- program arguments` и его start-result semantics.
Добавить --shutdown-timeout SECONDS, --on-timeout force|cancel,
--quit-command "command arguments" (существующий argv parser, не shell).
Аргументы после -- не изменять. Добавить session stop/cancel/status,
power reboot/poweroff. Status показывает provider, state, identity, apps,
effective shutdown method и результат.

Отсутствующий coordinator указанной generation — ошибка, не тихий fallback.
Без RSDM token сохранить старый базовый app launch в graphical target,
но координация/отмена в этом режиме явно недоступны.

Starting -> Running -> Preparing -> StoppingSession -> Closed.
Из Preparing возможен возврат в Running. Из StoppingSession отмены нет.

1. Закрыть launch gate.
2. Разрешить уже учтённые pending start jobs и получить полный app snapshot.
3. Параллельно отправить quit request каждому приложению.
4. Дождаться фактического завершения app processes/cgroup.
5. При timeout force дать оставшимся процессам до 5 секунд SIGTERM,
   затем SIGKILL; cancel прекращает logout до global teardown.
6. После app phase остановить owned autostart, graphical lifecycle и
   compositor, дождаться units, убрать только owned environment.

Managed WM method order: explicit quit command, XSMP client, SIGTERM main.
Native DE auto method остаётся штатным, без предварительного SIGTERM.
Command success/window disappearance/main PID exit не равны завершению
приложения. Таймауты используют monotonic clock и идут параллельно.

App services сохраняют ExitType=cgroup и получают synchronous ExecStop
backstop. Он исключает собственный PID/helper processes из ожидания,
не StopUnit самого себя и не ждёт собственный job. Единый сохранённый
deadline исключает повторные полные timeouts. Fallback escalation использует
pidfd для проверенных app processes, исключая helper. Начатый внешний unit
stop не отменяется. KillMode=control-group остаётся последней защитой.

## Питание, PAM, DM/Greeter/Lock

Для RSDM-controlled power: проверить действие logind, отменяемо подготовить
apps, запросить power от имени пользователя, при принятии завершить
lifecycle под inhibitor; при отказе сохранить compositor. Native DE power
делегируется единожды штатному manager.

Координатор заранее держит delay inhibitor и слушает PrepareForShutdown.
Внешний shutdown неотменяемый, бюджеты ограничены опубликованным delay
с резервом. Системные timeout/polkit параметры автоматически не менять.

SessionLauncher::start -> RunningSession; RunningSession::wait -> SessionExit.
Handle владеет ожиданием и root-side delay FD. Root owner ждёт user wrapper,
выполняет idempotent cleanup от имени UID для известной generation, закрывает
PAM, освобождает FD. После аварии coordinator cleanup имеет короткий budget.
Создавать root D-Bus connection после manual fork; не добавлять его фоновые
threads до fork. Сохранить detach leader и VT handshake.

DM владеет PAM/VT/wait. Greeter использует logind power request. Lock
асинхронно запрашивает reboot/poweroff и сохраняет opaque lock при отказе
или отмене. Suspend/hibernate не запускают logout.

Прямой compositor quit обходит preparation: документировать WM bindings
на session stop/power, но не переписывать пользовательские WM configs.
Process exit status и ShutdownReport разделены; exit 0 не доказательство save.

## Обязательный XSMP этап

Отдельный libSM/libICE module: local ICE, cookie authentication, запрет
host-based auth, проверка UID/PID/cgroup managed client. SESSION_MANAGER и
ICEAUTHORITY публикуются до app launch. Реализовать SaveYourself,
interaction, phase 2, отмену, Die и ожидание реального app exit.
SaveYourselfDone не означает выход процесса.

Не заменять native DE XSMP server и не принуждать приложения к XWayland.
Добавить --shutdown-method auto|term|xsmp. Explicit xsmp без поддержки —
понятная ошибка. Официальные пакеты после validation включают XSMP,
custom builds без feature явно показывают ограничение.

## Этапы и контроль выполнения

- [x] 1. Контракты/identity, config path/metadata, CLI separation, state/report.
- [x] 2. Typed systemd transport, argv/env, precise jobs, capability checks.
- [x] 3. WM/DE adapters, per-generation anchor, readiness, native niri.
- [x] 4. App registry/gate/policies, parallel preparation, cancellation.
- [x] 5. ExecStop backstop, bounded deadlines, cgroup/pidfd, recovery cleanup.
- [x] 6. Power/logind, root lifetime/PAM guard, asynchronous Lock actions.
- [x] 7. XSMP protocol/authentication/save/cancel/die.
- [x] 8. Packaging/docs/integration acceptance.

Meaningful checks: app flush while display/services alive; remaining children;
helper self-cgroup; launch/logout race and pending jobs; late finalize;
stale generation; cancel/commit; native DE dialogs; real native niri readiness;
PAM close after cleanup; DM restart survivor; coordinator crash; denied power;
external 5-second budget; lock stays opaque; XSMP phase2/cancel/auth.

Synthetic test programs in repository, isolated systemd/logind/WM VM tests.
No destructive tests on the current desktop. Validate packages for x86_64
and aarch64. Cargo tests/checks now allowed; never run forbidden tools.
Review complete diff and file/function sizes after each stage. Aim <300 lines
per module; >400 lines or >50-line functions need concrete justification.

## Рабочий журнал

- 2026-10-06: согласованный план сохранён; implementation goal создан.
  Исходный HEAD 55cdec9, версия 2.1.0. Рабочая копия до реализации чистая,
  кроме локальных AGENTS.md и исходного architecture document.
  До реализации выполнены read-only исследования и git diff --check;
  сборки и tests ещё не запускались.

- 2026-10-06: внедрены typed D-Bus transport и coordinator, команды
  stop/cancel/status/power, параметры rsdm app, generation recovery records,
  launch gate, parallel app preflight, synchronous ExecStop и pidfd escalation.
  Root launcher теперь возвращает lifetime handle; cleanup выполняется после
  setuid, inhibitor живёт до PAM close/end, что проверено отдельными тестами.
  Lock выполняет power request асинхронно; Greeter использует logind.
  Cargo check --workspace --locked и cargo test --workspace --locked прошли:
  CLI проверяется на отдельном dbus-daemon; тесты не обращаются к
  шине работающего desktop. Форматтеры и линтеры не запускались.
  Проверка CLI обнаружила default replacement flags zbus; имя coordinator
  теперь явно нельзя перехватить или заменить повторным запуском.
  Anchor использует Requisite/PartOf compositor вместо BindsTo, чтобы его
  активация не перезапускала завершившийся compositor. Stage 3–6 ещё требуют
  проверки аварийных сценариев и реального systemd в изолированной VM.
  XSMP, упаковка, MSRV 1.88 и итоговая acceptance matrix ещё впереди.

- 2026-10-06: отдельная NixOS VM с настоящими systemd/logind/PAM прошла
  первые три сценария: save-before-compositor, cancel timeout с живым
  приложением/compositor и внешний stop target с synchronous ExecStop.
  Тест описан в tests/session-lifecycle/default.nix; пока запускается с
  локальным debug binary, подключение к flake checks относится к stage 8.
  VM выявила duplicate XDG_SESSION_ID в SetEnvironment и RefuseManualStart
  у xdg-desktop-autostart.target. Используется существующая нормализация
  identity; autostart активируется Wants anchor без обратного After.
  При недоступном user delay inhibitor обычный login/logout сохраняется;
  внешний shutdown в таком окружении зависит от отдельного root PAM guard,
  о недоступности user guard пишется предупреждение. В VM для проверки
  штатной user policy включён polkit; специальных allow rules не добавлено.
  До окончательной готовности нужны дополнительные аварийные/launch-race
  сценарии, startup rollback, проверки native WM/DE и обязательный XSMP.

- 2026-10-07: в изолированной VM прошли 11 сценариев lifecycle и XSMP:
  literal argv, save-before-compositor, timeout cancel, stale generation,
  отдельный quit helper, восстановление после SIGKILL, внешний target stop,
  общий first-phase/phase-2 barrier, отмена через interaction, save failure,
  ожидание actual process exit после SaveYourselfDone/Die и explicit XSMP
  без peer. Результат: /nix/store/pag48igjpsvhqgdjqxczsl20lqfdr0n1-vm-test-run-rsdm-session-lifecycle.
  Дополнительно исправлены races NoSuchUnit и сброшенного InvocationID;
  actor error теперь удерживает lease при drain/recovery. Проверка XSMP
  authentication, реальный native niri, power acceptance, MSRV и упаковка
  ещё не завершены. Версия и remote остаются без изменений.

- 2026-10-07: настоящий niri-session/niri.service в отдельной VM с virtio GPU
  прошёл notify readiness, отсутствие временного compositor unit, cancellation
  с живым native compositor и app save до native stop. Результат:
  /nix/store/7kiqbdivfnflc8hadhi04f9bb3vdn3zf-vm-test-run-rsdm-native-niri.
  Cargo 1.88.0/Rust 1.88.0: check --workspace --all-targets --locked прошёл.
  Три private-bus power tests прошли: FD lifetime, initial shutdown и обе
  PrepareForShutdown notifications с бюджетом 4.75 s, отказ user inhibitor
  без потери notifications. Проверки не затрагивали шины текущего desktop.

- 2026-10-07: lifecycle VM расширена до 16 сценариев: аутентификация XSMP
  с неправильным cookie и с правильным cookie вне зарегистрированного cgroup,
  оставшиеся children, параллельные Force deadlines, поздняя Cancel во время
  SIGTERM grace и закрытый admission gate. Все прошли. Дополнительно добавлена
  проверка startup rollback до/после публикации session record; её итоговый
  запуск входит в финальную матрицу ниже.

- 2026-10-07: native niri VM прошла 3 сценария с исходным niri-session и
  настоящим Type=notify niri.service, без поддельного READY. Проверены IPC,
  отказ питания, Cancel и app save до native stop. Уточнение результата:
  lifecycle/readiness проверены, rendering niri в этом VM не подтверждён:
  его TTY backend отвергает software renderer. Результат:
  /nix/store/cbzvxmf08wbbx4mmrc0xshp5m0z9lbcv-vm-test-run-rsdm-native-niri.
  На реальном systemd/PAM отдельно прошли 3 сценария GNOME/Plasma D-Bus
  handoff и detached external launcher. Это protocol fixtures с upstream API,
  а не полноценные графические GNOME/Plasma. Результат:
  /nix/store/rcvzcyf9gfqyx9nad243mcygzwgspmmj-vm-test-run-rsdm-native-de-handoff.

- 2026-10-07: power VM прошла 5 сценариев: CanPowerOff denial, отказ самого
  запроса после app save, внешний shutdown/revoke, 5-second budget и закрытие
  inhibitor FD. Все запросы выполнены UID пользователя, root retry отсутствует.
  Logind protocol fixture использует отдельную шину и не выключает систему.
  На настоящем Sway/Pixman Lock после отказа logind сохранил PID и непрозрачные
  поверхности; pixels проверены по screenshots. Полный DM через Greeter/PAM
  прошёл restart survivor и logout: app save при живом display, PAM close,
  удаление logind session и возвращение Greeter. Результаты:
  /nix/store/zya2q3fbblk4phbq6ac5cpd5gcdhgss1-vm-test-run-rsdm-lock-power,
  /nix/store/fkw3vqyq70gap52ws7zpwsxkhcrcbcw1-vm-test-run-rsdm-dm-lifecycle.

- 2026-10-07: review исправил блокировку на FIFO recovery record/lease,
  idempotent cleanup после обнуления InvocationID у уже закрытого native unit,
  SIGTERM foreground external launcher, ограничение root recovery helper,
  невозможность продлить внешний deadline повторными notifications и cleanup
  пустой generation при startup validation failure. Main app signal теперь
  использует pidfd с повторной cgroup verification. Systemd и D-Bus activation
  очищаются вместе; failed activation update сохраняет ownership для retry.
  Удалён неиспользуемый ShutdownReason, handlers разделены по responsibility.

- 2026-10-07: финальные Cargo tests прошли: 218 тестов;
  cargo test --workspace --locked. Rust/Cargo 1.88.0:
  cargo check --workspace --all-targets --locked прошёл.
  cargo deny check: advisories/bans/licenses/sources ok; разрешённые policy
  warnings о duplicate dependencies остаются. Проверка stable publication:
  python3 -B packaging/nix/test_update_stable.py — 4 tests passed.
  git diff --check и bash -n обоих PKGBUILD прошли. Форматтеры/Clippy/actionlint
  не запускались. Nix source release package собран с libSM/libICE:
  /nix/store/21c5vnvdq9fj7nziqc8336hd8zfdz1ah-rsdm-2.1.0.
  nix flake check --all-systems --no-build прошёл для x86_64/aarch64.
  Итоговые шесть VM checks запускаются с этим release source package;
  aarch64 cross-build и no-default-features check ещё выполняются.

- 2026-10-07: итоговая acceptance завершена на текущем implementation.
  В XSMP выбор протокола фиксируется один раз на shutdown attempt: отдельные
  app workers больше не запускают повторный Save после cancellation.
  Interaction Cancel и save failure проверены с последующим успешным logout
  того же клиента; journal не содержит XSMP protocol errors. Native anchor
  требует активный graphical target и готовый manager; Requisite запрещает
  перезапустить native target при readiness/stop race. GNOME/Plasma fixtures
  проверяют готовый API при ещё неактивном target перед его native activation.

  На финальном коде прошли cargo test --workspace --locked — 218 tests;
  Rust/Cargo 1.88.0 cargo check --workspace --all-targets --locked;
  cargo check --locked -p rsdm --no-default-features. Cargo deny policy
  прошла; Cargo.lock после неё не изменялся. Также прошли
  python3 -B packaging/nix/test_update_stable.py — 4 tests,
  git diff --check и bash -n packaging/arch/PKGBUILD
  packaging/arch/rsdm-bin/PKGBUILD. Запрещённые инструменты не запускались.

  Nix source release build x86_64:
  /nix/store/74dzp6n42iv6kaxapbqsilbn1z02g5fn-rsdm-2.1.0.
  Aarch64 release cross-build с XSMP прошёл:
  /nix/store/g61w914xh023jr434427cf6gzwarh5gm-rsdm-cross-aarch64-unknown-linux-gnu-2.1.0;
  readelf -h подтвердил Machine: AArch64. FFI error buffers используют
  libc::c_char: его signedness различается между этими архитектурами.
  nix flake check --all-systems --no-build --no-write-lock-file прошёл
  на path snapshot /tmp/rsdm-session-source.rXsD2V, включающем новые файлы.

  nix build --no-link --no-write-lock-file --print-out-paths --max-jobs 2 -L
  для checks.x86_64-linux.{session-lifecycle,native-niri,native-de-handoff,
  power-lifecycle,lock-power,dm-lifecycle} того же snapshot прошёл.
  Все шесть VM используют итоговый source release package; 31 сценарий:
  17 lifecycle/XSMP, 3 native niri, 3 native DE/external, 5 power, 1 Lock,
  2 DM. Результаты:
  /nix/store/6vzrl20br1s793416yjc4y0dhcvj1dh9-vm-test-run-rsdm-session-lifecycle,
  /nix/store/5cgl2xx3y3nar46k425x03s6aixzwv3d-vm-test-run-rsdm-native-niri,
  /nix/store/s8bqkff9vgvkgb42ghyz9n5jmywfv136-vm-test-run-rsdm-native-de-handoff,
  /nix/store/cm4vdba1ng0vigjiny24nb31g9d0pblq-vm-test-run-rsdm-power-lifecycle,
  /nix/store/0mj8lacmrliz6qm59gdj2w5hf2b2c08x-vm-test-run-rsdm-lock-power,
  /nix/store/lpfrmmw7pdlyqfw0gxmrj1lgw53r8isn-vm-test-run-rsdm-dm-lifecycle.

  Full diff review завершён. Изменённые handwritten Rust modules не превышают
  300 строк. Reviewed function exceptions: Coordinator::create — 91 строка
  с явной инициализацией полей и упорядоченным получением ресурсов;
  существующие event loops run_greeter_loop — 64, Lock wayland::run — 69.
  Их механическое дробление затруднило бы проверку порядка ownership/lifetime;
  unrelated UI refactoring не выполнялся. Fixtures освобождены от size limits.
  Обязательных незавершённых этапов плана нет. Версия остаётся 2.1.0;
  commit/bump/push не выполнялись, AGENTS.md не staged и не изменён.

- 2026-10-07: по явному разрешению пользователя workspace и Cargo.lock
  обновлены до 2.2.0. По последнему указанию реализация публикуется одним
  коммитом. cargo check --workspace --all-targets --offline прошёл на 2.2.0.
  В локальном AGENTS.md закреплены отдельные коммиты завершённых задач;
  bump и push разрешены только по отдельному явному указанию пользователя.
  AGENTS.md остаётся вне Git.

## Границы результата

Нет автоматического управления всеми пользовательскими программами,
восстановления закрытых apps, нескольких graphical sessions одного UID,
backend без systemd или гарантии save у программы без поддерживаемого quit.
Power loss, SIGKILL и direct compositor quit остаются аварийными сценариями.
Runtime acceptance выполнена в x86_64 NixOS VM. Aarch64 hardware/runtime,
полноценные графические GNOME/Plasma и реальная отрисовка native niri
не проверены; проверены соответственно cross-build, native protocol fixtures
и настоящий notify/IPC/lifecycle niri. Гарантия корректного save для любой
программы невозможна без её участия в quit/save protocol; crash markers
Chromium/Mumble и других программ не подменяются.

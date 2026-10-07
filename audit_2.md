# Аудит RSDM

Дата: 2026-10-07. Исходный код не изменялся.

## Часть 1. Ошибки и риски выполнения

Ниже сохранены замечания первого прохода по DM, Greeter, Lock и Idle.
Риски, зависящие от окружения, отделены от воспроизведённых ошибок.

### 1. DM: общий дедлайн shutdown фактически не общий

Места: [coordinator_shutdown.rs:162](crates/rsdm-infra/src/session_manager/coordinator_shutdown.rs#L162),
[app_stop.rs:219](crates/rsdm-infra/src/session_manager/app_stop.rs#L219).

После подготовки приложений начинается последовательное завершение с отдельными
таймаутами: по 15 секунд на targets и anchor, дополнительные ожидания compositor
и logout-команды. После SIGKILL также выделяется ещё пять секунд независимо от
общего дедлайна. При системном shutdown RSDM может выйти за бюджет logind и не
успеть завершить cleanup и закрыть PAM. Всем этапам нужен один оставшийся бюджет.

### 2. Idle: после неудачного запуска Lock может не быть повторной попытки

Места: [seats.rs:74](crates/rsdm-idle/src/seats.rs#L74),
[lib.rs:146](crates/rsdm-idle/src/lib.rs#L146).

Ошибка запуска сбрасывает `lock_active`, но не уведомляет основной цикл и не
назначает повторную попытку. Пока пользователь отсутствует, нового `Idled` не
будет: он приходит после перехода из активности в бездействие. Единичный сбой
оставляет рабочий стол незаблокированным до следующего цикла активности.
Нужна повторная попытка с задержкой, пока seats остаются idle.

### 3. Idle: зависший on_unlock отключает последующую автоблокировку

Места: [hooks.rs:28](crates/rsdm-idle/src/hooks.rs#L28),
[lib.rs:118](crates/rsdm-idle/src/lib.rs#L118).

Hooks выполняются через блокирующий `.status()` без ограничения времени.
`lock_active` остаётся установленным до завершения hook. Если `on_unlock` завис,
экран уже открыт, но следующие попытки автоблокировки отклоняются как уже
работающий цикл. Жизненный цикл locker не должен зависеть от завершения hook.

### 4. Lock: secondary_output = off может оставить монитор выключенным

Места: [handlers.rs:111](crates/rsdm-lock/src/wayland/handlers.rs#L111),
[output.rs:107](crates/rsdm-lock/src/wayland/output.rs#L107).

При удалении Wayland-output его имя удаляется из `powered_off_outputs`. Но niri
при отключении монитора действительно удаляет соответствующий output, что видно
в [исходниках niri](https://github.com/niri-wm/niri/blob/main/src/niri.rs).
Обработчик забывает монитор, который затем должен включить. Это вывод по цепочке
вызовов; на живом compositor сценарий не запускался. Список отключённых RSDM
мониторов должен переживать удаление Wayland-объекта.

### 5. Lock: команды niri блокируют обработку Wayland без таймаута

Место: [output.rs:164](crates/rsdm-lock/src/wayland/output.rs#L164).

`niri msg …` запускается через `.status()` прямо из обработчиков событий и
восстановления outputs. Пока команда не завершилась, не обрабатываются клавиатура,
результаты PAM и emergency unlock. Зависший IPC превращается в зависший Lock.
Нужен отдельный worker с ограничением времени, как для power actions.

### 6. DM: безопасность второго fork зависит от поведения PAM/NSS

Места: [launcher.rs:108](crates/rsdm-infra/src/unix/launcher.rs#L108),
[launcher.rs:170](crates/rsdm-infra/src/unix/launcher.rs#L170).

Fork происходит после работы PAM. В дочернем процессе затем выполняются `setenv`,
`initgroups` и выделение памяти для `argv_ptrs()`. Если PAM или используемые
библиотеки создали потоки, операции после fork могут зависнуть на унаследованных
блокировках: [POSIX ограничивает этот участок async-signal-safe операциями](https://pubs.opengroup.org/onlinepubs/7908799/xsh/fork.html).
Это условный риск, а не воспроизведённое зависание. Отсутствие собственного D-Bus
executor не гарантирует однопоточность процесса.

### 7. DM: управляющий цикл менеджера сессий блокируется на D-Bus

Места: [coordinator_observation.rs:10](crates/rsdm-infra/src/session_manager/coordinator_observation.rs#L10),
[bus_connection.rs:58](crates/rsdm-infra/src/session_manager/bus_connection.rs#L58).

Наблюдение за процессами, readiness и удаление завершённых apps выполняют
синхронные запросы прямо в actor. Во время медленного запроса не обслуживается
`Cancel`. Reconnect дополнительно удерживает общий `Mutex` через сетевые `.await`,
блокируя остальных workers. Вынос start/stop в потоки не обеспечивает
отзывчивость всего управления.

### 8. DM: количество рабочих потоков менеджера сессий не ограничено

Места: [coordinator_requests.rs:146](crates/rsdm-infra/src/session_manager/coordinator_requests.rs#L146),
[app_stop.rs:56](crates/rsdm-infra/src/session_manager/app_stop.rs#L56).

Каждый launch создаёт новый поток; shutdown создаёт по потоку на приложение.
`workers` только считает их. Очередь запусков во время старта также не ограничена.
При массовом автозапуске нагрузка растёт вместе с количеством запросов.
Нужен предел одновременных операций и ограниченная очередь.

### 9. DM / Greeter: разбор desktop entry портит аргументы запуска

Место: [desktop_entry.rs:80](crates/rsdm-infra/src/sessions/desktop_entry.rs#L80).

`%c` и `%k` удаляются вместо подстановки имени и пути desktop-файла; неизвестные
коды сохраняются, хотя [спецификация требует отклонять их](https://specifications.freedesktop.org/desktop-entry/latest-single/#exec-variables).
Воспроизведено на собранной библиотеке:
`session-launcher --desktop-file %k` превращается в
`["session-launcher", "--desktop-file"]`.
Это неверный argv, способный сорвать запуск сессии.

### 10. Lock: каждый кадр заново обрабатывает весь экран

Места: [render.rs:88](crates/rsdm-lock/src/wayland/render.rs#L88),
[render.rs:158](crates/rsdm-lock/src/wayland/render.rs#L158),
[lib.rs:44](crates/rsdm-lock/src/lib.rs#L44).

Создаётся новый Canvas, заново масштабируются и затемняются обои, создаётся
shm-buffer и копируется весь framebuffer. Перерисовываются даже чёрные вторичные
outputs. Расчёт для одного 4K-монитора: около 33 МБ на буфер и около 500 МБ/с
только на копирование при 15 FPS. Производительность не измерялась.
Стоит переиспользовать buffers и кэшировать статический фон.

### 11. Lock: часы замирают при статическом фоне

Место: [mod.rs:219](crates/rsdm-lock/src/wayland/mod.rs#L219).

Таймаут polling сам по себе не вызывает redraw: периодическое обновление
назначается только для анимированного фона. При `background = "none"` время
остаётся старым до ввода или другого события Wayland. Обновлению часов нужен
собственный таймер.

### 12. Greeter: восстановление console status подменяет системную настройку

Места: [terminal.rs:129](crates/rsdm-tui/src/screens/login/interactive/terminal.rs#L129),
[terminal.rs:142](crates/rsdm-tui/src/screens/login/interactive/terminal.rs#L142).

Guard запоминает успешную отправку сигнала отключения, но не прежнее состояние
systemd. При выходе он всегда отправляет сигнал включения. Если вывод статусов
был изначально отключён администратором, после работы Greeter он включится.
Восстановление должно возвращать исходную политику.

## Проверки и ограничения первого прохода

- `nix develop --no-update-lock-file --command cargo test --locked --offline --workspace` — прошла.
- Разбор `%c`, `%k` и неизвестного кода проверен временной Rust-программой вне репозитория.
- Живые сценарии VT, PAM, shutdown и Wayland не запускались.
- Исходный код, конфигурация и зависимости не изменялись.
- `crates/rsdm-core/src/domain/config/validation.rs` содержит 463 строки, включая тесты; производственная часть занимает 285 строк.

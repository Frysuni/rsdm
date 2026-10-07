# Аудит RSDM

Дата: 2026-10-07. В рамках этого аудита исходный код не изменялся.

## Часть 1. Ошибки и риски выполнения

Ниже сохранены замечания первого прохода по DM, Greeter, Lock и Idle.
Риски, зависящие от окружения, отделены от воспроизведённых ошибок.
Это исторический список первого прохода. Актуальность этих пунктов после
последующих параллельных исправлений здесь повторно не проверялась.

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

## Часть 2. Спорные способы реализации

Второй проход: проверены реализации и вызывающий код, затем профильные API и
библиотеки. Базовый HEAD при оформлении — `aa28004`; рабочее дерево менялось
параллельно. P1 — существенный риск или прямое нарушение заданного ограничения;
P2 — недостаток подхода, совместимости или сопровождения. Это предложения для
обсуждения; исправления исходников и добавление зависимостей не выполнялись.
Числовые константы ABI, имена D-Bus интерфейсов и обязательные значения
протоколов сами по себе не считаются плохим хардкодом.

### 13. P1 / Idle: hooks являются shell-программами

Места: [hooks.rs:28](crates/rsdm-idle/src/hooks.rs#L28),
[idle.rs:20](crates/rsdm-core/src/domain/config/idle.rs#L20),
[rsdm.toml:128](rsdm.toml#L128).

`on_lock` и `on_unlock` передаются в `sh -c`. Это прямо нарушает требование
не писать shell-скрипты в коде. Строка команды одновременно задаёт программу,
аргументы, подстановки и перенаправления; кавычки и окружение становятся частью
поведения. Исполнение документировано как доверенная конфигурация, поэтому
это не доказанная инъекция со стороны постороннего пользователя.

Предпочтительный путь: список argv для каждого hook и прямой `Command`.
У `idle.lock_command` уже есть такой формат. Переход потребует согласованного
изменения конфигурации, корневого примера, Nix options и документации; цепочки
действий представляются несколькими командами. Ограничение времени исполнения
нужно независимо от отказа от shell.

Shell также исполняется в тестах [session_cleanup.rs:48](crates/rsdm-infra/src/unix/session_cleanup.rs#L48)
и генерируется в [hooks.rs:49](crates/rsdm-idle/src/hooks.rs#L49).
Нужен тестовый процесс на Rust с заданным exit code/остановкой и записью маркера.
Строки `sh -c` в [command.rs:131](crates/rsdm-infra/src/unix/command.rs#L131)
не исполняются, но их можно заменить нейтральными примерами проверки кавычек.

### 14. P1 / DM: команда getty восстанавливается из необратимого текстового вывода

Места: [getty_query.rs:43](crates/rsdm-infra/src/unix/getty_query.rs#L43),
[getty_query.rs:71](crates/rsdm-infra/src/unix/getty_query.rs#L71),
[fallback.rs:32](crates/rsdm-infra/src/unix/fallback.rs#L32).

RSDM запускает `systemctl show`, извлекает `argv[]=`, самостоятельно разбирает
кавычки, заменяет часть specifiers и удаляет `$VAR`. Но systemctl соединяет argv
пробелами без восстановления кавычек: один аргумент `-p -- \\u` неотличим от
нескольких. Тест здесь содержит вручную добавленные кавычки, которых такой
вывод не гарантирует. Исправлением tokenizer потерянные границы не вернуть.
Кроме того, удаление переменных не воспроизводит окружение штатного getty.
Это подтверждается [кодом systemctl](https://github.com/systemd/systemd/blob/main/src/systemctl/systemctl-show.c).

Предпочтительный путь: согласовать с packaging передачу VT штатному getty unit
либо формировать известный argv через уже существующий `dm.fallback.command`.
Если обнаружение команды всё же нужно, читать типизированное свойство `ExecStart`
по D-Bus через существующий zbus: argv там является массивом строк.
[API systemd](https://github.com/systemd/systemd/blob/main/man/org.freedesktop.systemd1.xml)
Необходимо отдельно решить раскрытие окружения и условия запуска: typed argv
сам по себе не переносит семантику unit. Сейчас Nix packaging маскирует getty
и autovt ([system.nix:42](packaging/nix/system.nix#L42)); простой `StartUnit`
без изменения схемы владения VT не подходит.

### 15. P1 / DM → Greeter: desktop entry проходит через два неполных парсера

Места: [desktop_entry.rs:53](crates/rsdm-infra/src/sessions/desktop_entry.rs#L53),
[desktop_entry.rs:80](crates/rsdm-infra/src/sessions/desktop_entry.rs#L80),
[command.rs:57](crates/rsdm-infra/src/unix/command.rs#L57).

Сначала вручную разбираются значения desktop entry и удаляются field codes,
затем `Session.exec` снова разбирается как строка команды. Теряется контекст
`Name`, пути файла и локали, нужный для раскрытия `%c`/`%k`; правила escaping
распределены между двумя модулями. Выбор первого локализованного `Name` при
отсутствии базового также не учитывает текущую локаль. Новые заплатки увеличат
собственную реализацию [Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry/latest-single/).

Предпочтительный путь: профильный parser для метаданных, затем один раз получить
проверенный argv с раскрытием допустимых field codes и передать его до exec
без обратного склеивания в строку. Правила допуска сессий остаются в DM.
`freedesktop-desktop-entry` — кандидат для метаданных, но его `parse_exec()`
в проверенной версии 0.8.3 использует `split_ascii_whitespace` и имеет неполное
escaping: это не готовая безопасная замена всей цепочки. Сначала нужны проверки
кавычек, backslashes, локалей и неизвестных field codes.
[Проверенная реализация библиотеки](https://docs.rs/freedesktop-desktop-entry/0.8.3/src/freedesktop_desktop_entry/exec.rs.html)

### 16. P1 / Lock: энергосбережение реализовано изменением топологии niri

Места: [output.rs:73](crates/rsdm-lock/src/wayland/output.rs#L73),
[output.rs:164](crates/rsdm-lock/src/wayland/output.rs#L164).

Режим `secondary_output = off` жёстко привязан к `NIRI_SOCKET`, бинарнику niri
и операции `output off`. Эта операция полностью отключает output; Lock затем
сам восстанавливает его. Так временная политика блокировки вмешивается в
расположение мониторов и обработку hotplug, откуда возникает риск пункта 4.
Даже сохранение имени уничтоженного wl_output не устранит саму смену топологии.
[Семантика niri output off](https://github.com/niri-wm/niri/blob/main/docs/wiki/Configuration%3A-Outputs.md#off)

Предпочтительный путь: управлять питанием через профильный
[wlr-output-power-management](https://github.com/swaywm/wlr-protocols/blob/master/unstable/wlr-output-power-management-unstable-v1.xml)
там, где compositor объявляет его поддержку, сохраняя прежний режим питания.
Протокол экспериментальный и доступен не везде; универсальную поддержку
обещать нельзя. Если безопасного per-output power API нет, использовать уже
имеющийся чёрный фон. При сохранении интеграции niri вынести её из Wayland UI
и использовать ограниченный по времени IPC; замена subprocess на socket
решает транспорт, но не превращает `output off` в энергосбережение.

### 17. P2 / DM и Lock: NSS lookup повторно реализован через unsafe libc

Места: [user.rs:46](crates/rsdm-infra/src/unix/user.rs#L46),
[group.rs:33](crates/rsdm-infra/src/unix/group.rs#L33),
[util.rs:17](crates/rsdm-lock/src/util.rs#L17).

Ручные циклы `getpwnam_r`, `getpwuid_r`, `getgrnam_r`, `getgrouplist` обслуживают
буферы, ERANGE и C-указатели. DM и Lock отдельно задают одинаковые лимиты
и преобразования, а ошибки Lock сворачивает в `None`. Здесь повторяется
обычная системная обвязка, а не специфическая логика display manager.

Предпочтительный путь: общий NSS adapter в infra на безопасных API
[nix::unistd::User](https://docs.rs/nix/latest/nix/unistd/struct.User.html),
[Group](https://docs.rs/nix/latest/nix/unistd/struct.Group.html) и
[getgrouplist](https://docs.rs/nix/latest/nix/unistd/fn.getgrouplist.html).
Сохранить различие между отсутствующим пользователем и отказом backend,
проверить ресурсные ограничения библиотеки. Политики DM (`deny_root`, groups)
и Lock остаются раздельными; Lock по-прежнему берёт identity из effective UID,
а не из `USER`. Одной заменой NSS нельзя устранить риски после fork из пункта 6.

### 18. P2 / DM и Lock: несколько собственных реализаций атомарной записи

Места: [storage/mod.rs:43](crates/rsdm-infra/src/storage/mod.rs#L43),
[save.rs:144](crates/rsdm-infra/src/config/save.rs#L144),
[snapshot.rs:16](crates/rsdm-infra/src/console_font/snapshot.rs#L16).

Отдельно реализованы создание temp file, имя, запись, rename и cleanup.
Политики уже расходятся: remembered state и config синхронизируются, font
snapshot нет; первые используют counter/time, snapshot — только PID.
Потребителям приходится сопровождать несколько вариантов одного механизма.
Отсутствие fsync у восстанавливаемого runtime snapshot само по себе не ошибка,
но выбор durability должен быть явным.

Предпочтительный путь: одна реализация атомарной замены в infra на
[atomic-write-file](https://docs.rs/atomic-write-file/latest/atomic_write_file/)
или эквивалентном проверенном механизме. Библиотека работает с временным файлом
и directory descriptors; права 0600/0644 и политика сохранения metadata
задаются явно. Доверие к каталогам, ownership, symlinks и блокировки конкурирующих
записей остаются отдельными требованиями. ACL/xattrs/SELinux она автоматически
не сохраняет; нельзя считать замену библиотеки полным решением безопасности.

### 19. P2 / DM: дефолтный поиск сессий содержит политику конкретного дистрибутива

Места: [session.rs:3](crates/rsdm-core/src/domain/config/session.rs#L3),
[discoverer.rs:37](crates/rsdm-infra/src/sessions/discoverer.rs#L37),
[rsdm.toml:47](rsdm.toml#L47).

Core безусловно ставит NixOS-путь первым, затем `/usr/share`, затем
`/usr/local/share`. Discoverer сохраняет первый ID и маскирует следующие,
поэтому локальный административный override проигрывает системному файлу.
При этом привычный [порядок XDG](https://specifications.freedesktop.org/basedir/latest/)
для системных data directories — `/usr/local/share`, затем `/usr/share`.

Предпочтительный путь: общий дефолт с ожидаемым приоритетом, а NixOS-пути
задавать packaging через существующий `dm.session_dirs`. Для root DM нельзя
бездумно подключать пользовательский `XDG_DATA_HOME` или доверять произвольным
путям из окружения. Вопрос здесь в приоритете и расположении distro policy,
а не в необходимости сделать любой системный путь настраиваемым.

### 20. P2 / Lock: доступность Hibernate определяется по sysfs вместо logind

Места: [input.rs:177](crates/rsdm-lock/src/wayland/input.rs#L177),
[wayland/mod.rs:277](crates/rsdm-lock/src/wayland/mod.rs#L277),
[power.rs:30](crates/rsdm-infra/src/power.rs#L30).

UI разрешает Hibernate при наличии `disk` в `/sys/power/state`, но сам запрос
идёт через logind. Возможность ядра не выражает административную политику,
настройку hibernation и право текущего пользователя. Проверка получается
отдельной неполной моделью возможностей того же backend.

Предпочтительный путь: брать capability у logind через уже имеющийся power
adapter. [CanHibernate](https://github.com/systemd/systemd/blob/main/man/org.freedesktop.login1.xml)
различает поддержку, запрет и необходимость авторизации; эти состояния
не стоит сворачивать в проверку одного слова sysfs. Запрос выполнять вне
Wayland event loop, с timeout; непосредственно перед действием backend
всё равно проверяет актуальную политику.

### 21. P2 / Greeter и Lock: общий renderer выполняет OS lookup на каждом кадре

Места: [banner.rs:25](crates/rsdm-ui/src/banner.rs#L25),
[banner.rs:110](crates/rsdm-ui/src/banner.rs#L110),
[interactive/mod.rs:62](crates/rsdm-tui/src/screens/login/interactive/mod.rs#L62),
[render.rs:133](crates/rsdm-lock/src/wayland/render.rs#L133).

Построение scene вызывает чтение hostname и, при соответствующем title source,
os-release, а также заново генерирует FIGlet art. Общий UI связывает отрисовку
с Linux filesystem и разбором системных метаданных. Parser os-release только
снимает двойные кавычки: не поддерживает одинарные, escaping и fallback
на `/usr/lib/os-release`, предусмотренный
[форматом os-release](https://github.com/systemd/systemd/blob/main/man/os-release.xml).

Предпочтительный путь: получить метаданные через infra при построении context,
передавать готовый текст в UI, пересоздавать title art при изменении текста,
font или дизайна. Часы остаются динамическими. Hostname получать системным API;
os-release читать профильным parser без исполнения shell. Это улучшает границы
модулей и убирает повторное I/O; величина ускорения не измерялась.

### 22. P2 / Lock: персональные настройки сохраняются в общий конфиг DM

Места: [config/mod.rs:10](crates/rsdm-infra/src/config/mod.rs#L10),
[input.rs:50](crates/rsdm-lock/src/wayland/input.rs#L50),
[save.rs:36](crates/rsdm-infra/src/config/save.rs#L36),
[save.rs:124](crates/rsdm-infra/src/config/save.rs#L124).

Кнопка Save переписывает исходный конфиг, по умолчанию `/etc/rsdm.toml`.
Обычному пользователю он недоступен для записи; для NixOS сохранение блокируется
по префиксу `/nix/store`. Чтобы сохранять оформление, пользователю приходится
предварительно создавать собственную копию общего конфига и передавать её
через `--config`. Жизненный цикл пользовательского оформления смешан
с административной конфигурацией DM, Greeter и политик безопасности.

Предпочтительный путь: системный конфиг оставить источником административных
настроек, а изменения оформления Lock сохранять отдельно в пользовательском
файле по [XDG](https://specifications.freedesktop.org/basedir/latest/).
Наложение должно разрешать только уже сохраняемые visual settings; настройки
DM и security пользовательский файл переопределять не должен. Это изменение
контракта сохранения, его нужно обсуждать отдельно, без повышения привилегий
Lock ради записи темы и без привязки механизма к `/nix/store`.

## Проверки и ограничения второго прохода

- Выполнен статический разбор указанных модулей, вызывающего кода и packaging; проверены профильные первичные источники API и реализации библиотек.
- Shell-поиск: `rg -n 'Command::new\("(sh|bash|dash|zsh)"\)|"(sh|bash) -c|printf unlocked|\$\$' crates`.
- Проверены локальные ссылки и диапазон указанных номеров строк; `git diff --check -- audit_2.md` и staged diff check.
- Сборка и тесты во втором проходе не запускались: изменён только отчёт. Результаты первого прохода выше не являются проверкой нового HEAD.
- Способы миграции конфигурации, новые зависимости и работа с живыми VT/Wayland/PAM отложены; рекомендации не проверялись интеграционным запуском.
- Из прочитанных исходников выше ориентира 300 строк: `config/save.rs` — 343, `screen/layout.rs` — 350, `config/validation.rs` — 463 (с тестами). `save_lock_runtime_settings` — 51 строка с сигнатурой, выше ориентира 50. Размеры указаны на момент прохода; эти файлы не менялись в рамках аудита.

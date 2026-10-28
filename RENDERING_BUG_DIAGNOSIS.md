# TanatoGothci — Диагностический отчёт по багам рендеринга

Расследование (только диагностика, код не менялся). Окно 420×320. Скриншот:
`.userfiles/chat/draft-522f031f-4ea6-41a0-a868-08765b2b3093/screenshot-20261028-073400-1.png`.

## Как устроен рендер сейчас (факты из кода)

- `src/index.html:54` — Reasoning-экран: `<pre id="reasoning-log" class="reasoning-log">` внутри `#screen-reasoning`.
- `src/index.html:59` — ASCII-экран: `<pre id="ascii-art" class="ascii-art">` внутри `#screen-ascii`.
- `src/styles.css:110-117` — **одно общее CSS-правило** на обе панели:
  ```css
  .reasoning-log, .ascii-art {
    margin: 0;
    white-space: pre-wrap;   /* ← перенос по всем `\n` И по ширине */
    word-break: break-word;  /* ← ломает слова по символам у края контейнера */
    font-family: ui-monospace, "Cascadia Mono", Consolas, "Courier New", monospace;
    font-size: 12px; line-height: 1.35;
  }
  ```
- `src/main.js:21-27` — `append(lineEl, text, cls)` создаёт **`<div>` и вставляет внутрь `<pre>`** (невалидная вложенность блока в `pre`), затем `textContent`.
- `src/main.js:102-105` — `reasoning:delta` → `append($("reasoning-log"), …)`.
- `src/main.js:111-113` — `ascii:frame` → `$("ascii-art").textContent = payload.text`.
- Бэкенд `src-tauri/src/session.rs:186-217` разводит потоки корректно: `Reasoning` → `reasoning:delta`, `DisplayAsciiArt` → `ascii:frame` (+ `tool:call`).

---

## Симптом 1 — reasoning переносится примерно через ~8 символов

Прямой «фиксированной узкой ширины ~8 символов» в CSS нет — контейнер `.screen`
(block + `flex:1` + `padding:10px` внутри `.content` 420px) даёт `#reasoning-log`
примерно 400px ширины. Значит «перенос через ~8 символов» — это не физическая
узость, а следствие комбинации:

1. `white-space: pre-wrap` на `<pre>` **рендерит каждый `\n` из стримингового
   текста как перенос строки**. Reasoning-дельта от модели приходит мелкими
   фрагментами, часто содержащими переводы строк / короткие «мысли»; `pre-wrap`
   пишет их в столбик → визуально текст «переносится» очень часто.
2. `word-break: break-word` **ломает слова посимвольно у правого края**
   контейнера — если поток присылает длинное слово/токен, он режется примерно
   через 7–9 символов `12px`-моноширины на строку. Это и даёт характерный
   «перенос через ~8 символов», не зависящий от реальной ширины панели.
3. `append()` кладёт блоки `<div>` внутрь `<pre>` — браузер обрабатывает каждый
   блок отдельно, что дополнительно ломает преформатирование/ширину строк.

**Куда чинить (на будущее):** `src/styles.css:112-113` — для reasoning-лога
убрать/ослабить агрессивный перенос: `white-space: pre-wrap` оставить только если
нужно сохранять `\n`, но `word-break: break-word` для связного текста убрать или
заменить на `overflow-wrap: break-word` (мягкий, по словам). Заодно
`src/main.js:append()` — писать текст не блочными `<div>` внутрь `<pre>`, а
евентообработчики должны класть в один элемент или использовать обычный
`<div>`, а не `pre`.

---

## Симптом 2 — ASCII-контент в reasoning-панели, ASCII-панель пустая

Важно: **прямой перестановки событий не найдено** — `ascii:frame` в
`src/main.js:111-113` пишется именно в `#ascii-art`, а `reasoning:delta` — в
`#reasoning-log`. То есть «путаницы направлений» (ascii-фрейм в reasoning-контейнер)
в текущем коде нет. Симптом складывается из двух реальных причин:

1. **Обе панели рендерятся по одному CSS-правилу (`white-space: pre-wrap` +
   `word-break: break-word`, `src/styles.css:110-117`), но ASCII-арт требует
   `pre` + `monospace` + **без переноса** (см. спец. §10.3, R-005).** На
   Reasoning-панели этот общий стиль ломает и её, а на ASCII-панели `pre-wrap` +
   `break-word` коверкают картинку. Однако «пустоты» самой ascii-панели это не
   объясняет.
2. **По спец. §10.2 и §4.4 вся текстовая «рассуждательная» выдача (включая
   `delta.content` для non-reasoning моделей и события инструментов типа
   `[tool] display_ascii_art called`) маршрутизируется в reasoning-лог.** Если
   модель рисует ASCII прямо в выдаче (обычным text/reasoning, а не вызовом
   инструмента `display_ascii_art`), этот текст попадает в **reasoning-панель**, —
   выглядит как «ASCII отобразился не там».
3. `ascii:frame` шлётся только когда модель **действительно вызвала инструмент**
   `display_ascii_art` (`src-tauri/src/session.rs:208-218`). Если вызов
   инструмента не происходит (модель рисует арт текстом, или стрим идёт только
   `Raw`/`Reasoning`/`Tool` без `DisplayAsciiArt`), `ascii-art.textContent` ни разу
   не устанавливается → ASCII-панель остаётся **пустой**.

Итог: «ascii в reasoning + пустая ascii-панель» = контент-уровневая
маршрутизация (всё текстовое идёт в reasoning-лог как catch-all), а не
перестановка event-id. ASCII-панель наполняется только по выделенному событию
инструмента.

**Куда чинить (на будущее):**
- `src/styles.css`: разделить правила — ASCII: `#ascii-art { white-space: pre;
  word-break: normal; overflow: auto; }` (моноширинный pre без переноса, R-005);
  reasoning: отдельное правило без `word-break: break-word`.
- `src/main.js:111-113`: ASCII-панель уже читает `ascii:frame` — корректно;
  дополнительно можно показывать fallback: если в контент-канале обнаружен
  арт, направлять его в `#ascii-art` (например, эмитить `ascii:frame` из
  reasoning-фрагмента). Бэкенд `src-tauri/src/session.rs` — место, где решать,
  что считать ASCII-фреймом: помимо `DisplayAsciiArt` можно вычленять арт из
  `delta.content`/`reasoning_content`, чтобы ascii-панель не пустовала.

---

## Резюме

| Симптом | Первопричина | Файл:строки | Фикс |
|---|---|---|---|
| Reasoning переносится через ~8 симв. | `word-break: break-word` + `pre-wrap` на `pre`; дивы в `pre` | styles.css:110-117; main.js:21-27,102-105 | мягкий перенос/убрать `break-word`; не класть блоки в `pre` |
| ASCII в reasoning, ascii-панель пуста | Общий стиль ломает ascii; весь текст/выдача идёт в reasoning (catch-all); `ascii:frame` только при вызове инструмента | styles.css:110-117; main.js:111-113; session.rs:186-217 | Отдельный `white-space: pre` для ascii; направлять распознанный арт в ascii-панель |

Напрямую `event-id`-перестановки (ascii-фрейм в reasoning-лог) в текущем коде
нет; симптом 2 объясняется контент-маршрутизацией и общим (некорректным для
ascii) CSS-правилом.

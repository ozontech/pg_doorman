# pg_doorman Web UI — Design System

Дата: 2026-05-06.
Дополнение к `2026-05-06-web-ui-design.md`. Фиксирует визуальный язык до начала кодинга, чтобы не выбирать каждый раз заново шрифт, цвет границы или плотность таблицы.

## 1. Контекст и аудитория

Целевой пользователь — DBA, SRE или dev-on-call в момент инцидента или регулярного оперативного обзора. Контекст использования:
- Десктоп, монитор большой, окно браузера часто не во весь экран — сетка должна работать от 960 px ширины.
- Сценарий — быстро понять, что происходит, и принять решение, идти ли в `psql` admin. UI **только показывает**, не правит.
- Часто открыт рядом с Grafana, терминалом, slack — должен визуально стоять рядом, не сливаться и не кричать.

Из этого следуют два решения, остальное — производное от них:
1. **Density над air-space.** Плотная сетка, мало padding, числа в моно. Свободное пространство тратим только на разделение блоков, не на «дыхание» внутри.
2. **Dark theme primary.** Operational tools чаще включают ночью на инциденте; dark — нейтральнее на длинной сессии и не слепит. Light theme — follow-up после MVP.

## 2. Aesthetic direction

**Industrial / utilitarian с нотами brutally minimal.**

Что это значит конкретно:
- Минимум декоративных элементов: никаких градиентов, теней, glassmorphism, скруглённых карточек с большими радиусами.
- Тонкие границы 1 px, тонкие разделители, большая часть «компонентов» — это таблицы и числа.
- Один акцентный цвет (cyan), используется только для интерактивности (focus, active, primary CTA-equivalent типа кнопки «Reset history»), не для декора.
- Семантические цвета (green/amber/red) применяются точечно — статусные badge, alerts, error-баннер. Не используем как декор.
- Никакого drop-shadow на ровном UI; box-shadow допустим только для overlay-элементов (modal, dropdown).
- Все числа — в табличных моно-цифрах. Это сигнатурный признак инструмента: «это DBA-tool, тут важна каждая цифра».

**Memorable element:** на каждой странице — таблица или плотная грид-метрика, где числа выровнены справа в IBM Plex Mono с tabular figures. UI узнаётся именно по этому, как Grafana по графикам или Linear по shortcut-меню.

**Чего избегаем (анти-слоп для нашего контекста):**
- Inter, Roboto, Arial, system-ui как primary шрифт.
- Фиолетовый/синий gradient на белом.
- «Облачные» иллюстрации, маскоты, эмодзи.
- Иконки с заливкой (filled). Только outline 1.5 stroke.
- Border-radius > 6 px на любом контейнере.
- Полупрозрачные blurred backgrounds.

## 3. Typography

### 3.1 Семейства

| Роль | Шрифт | Источник | Fallback |
|---|---|---|---|
| UI / sans | IBM Plex Sans (400, 500, 600) | self-host woff2, ~30 KB на вес | system-ui, sans-serif |
| Mono / numbers / logs / SQL | IBM Plex Mono (400, 500) | self-host woff2, ~25 KB на вес | ui-monospace, monospace |

Self-host обязателен: бинарь embed'ит SPA через `include_dir!`, никаких внешних CDN-загрузок. Веса загружаем только реально используемые (две на каждое семейство = 4 файла, ≈110 KB total после woff2-сжатия).

IBM Plex выбран осознанно:
- Distinctive, не Inter — проходит anti-slop фильтр.
- Хорошо читается в плотной сетке, рассчитан на data-tables.
- Mono-вариант имеет правильные tabular figures и slashed zero — нужно для логов и счётчиков.
- Open-source (SIL OFL), без лицензионных проблем.

### 3.2 Шкала размеров

```css
--font-size-xs:   11px  /* table secondary, axis labels, badge */
--font-size-sm:   13px  /* default table cell, secondary text */
--font-size-base: 14px  /* body, sidebar items */
--font-size-md:   16px  /* page section title */
--font-size-lg:   20px  /* page H1, hero metric label */
--font-size-xl:   28px  /* hero metric number в Overview */
```

Мы намеренно **не** идём по 16 px base из mainstream-гайда — это operational density UI, аналог Bloomberg terminal'а или Datadog'а, где 13 px sans + 12 px mono норма. 14 px base — уважительный компромисс, читается без напряжения, но не съедает экран.

Line-height: 1.4 для UI-текста, 1.5 для логов (читабельность многострочных сообщений), 1.0 для hero metric.

Letter-spacing: `0.01em` для caps-меток («ACTIVE», «PAUSED», «TLS»). Везде остальном — default.

### 3.3 Веса

- 400 (regular) — body, table cell, log entry.
- 500 (medium) — table header, sidebar active item, badge, page H1.
- 600 (semibold) — hero metric number в Overview, alerts.

Никаких 300/700/900 — три веса хватит, больше создаёт визуальный шум.

### 3.4 Tabular figures

Везде, где число читается в таблице или счётчике — `font-variant-numeric: tabular-nums slashed-zero`. Применяется через CSS-класс `tabular`, который ставится на `<td>` числовых колонок и на hero-metric.

## 4. Color

### 4.1 Палитра (dark, primary)

```css
:root {
  /* Surface */
  --bg:           #0a0d12;  /* page background */
  --surface:      #11151c;  /* cards, table bg */
  --surface-2:    #161b24;  /* hover, active row */
  --surface-3:    #1c2230;  /* dropdown, modal */

  /* Border */
  --border:       #232a36;  /* default border, table rows */
  --border-strong:#2d3543;  /* card edges, focus outline */

  /* Text */
  --text:         #e6e9ee;  /* primary */
  --text-muted:   #8a93a4;  /* secondary, table headers */
  --text-dim:     #5a6275;  /* tertiary, captions, disabled */

  /* Accent */
  --accent:       #22b8cf;  /* cyan-500-ish, primary interactive */
  --accent-hover: #3ec8d9;
  --accent-fg:    #042024;  /* text on accent fill */

  /* Semantic */
  --success:      #2dc26b;
  --warning:      #f5a524;
  --danger:       #e5484d;
  --info:         #5b8cff;

  /* Chart palette (uPlot lines) */
  --chart-1: #22b8cf;  /* primary */
  --chart-2: #2dc26b;  /* secondary positive */
  --chart-3: #f5a524;  /* tertiary / warning curve */
  --chart-4: #b18cf5;  /* quaternary, used sparingly */
}
```

Проверка контраста (WCAG AA):
- text on bg: 13.8:1 ✓
- text-muted on bg: 6.4:1 ✓
- text-dim on bg: 4.0:1 ✓ (хватает для tertiary captions, не для body)
- accent on bg: 7.6:1 ✓
- accent-fg on accent fill: 7.1:1 ✓

### 4.2 Использование акцента

Один акцент — cyan. Намеренно не amber: amber мы оставили под warning, чтобы не было коллизии «акцент совпал с предупреждением». Cyan distinctive, не повторяет ни Linear-фиолетовый, ни Grafana-оранжевый, ни GitHub-зелёный.

Где появляется accent:
- Focus ring всех интерактивных элементов (`outline: 2px solid var(--accent); outline-offset: 2px`).
- Active sidebar item (текст + 2 px левая полоса).
- Hover на ссылках в таблицах.
- Sparkline primary curve в Overview.
- Tab underline (2 px под активной вкладкой).

Где accent **не** появляется: декоративные badge, заголовки, фон карточки, иконки в покое.

### 4.3 Light theme (follow-up)

Зарезервируем переменные с теми же именами для `:root.light`. Конкретные значения подбираем после MVP — сейчас не тратим время.

## 5. Layout и плотность

### 5.1 App shell

```
┌──────────────────────────────────────────────────────────────────────┐
│ Top bar (48 px):                                                     │
│  pg_doorman v3.8.0  [● OK] 12 pools | 0 paused | 0.20 err/s          │
│                                          Updated 0.8s ago    [admin] │
├──────────┬───────────────────────────────────────────────────────────┤
│ Sidebar  │  Page header (56 px: title + actions + breadcrumbs)       │
│ 220 px   ├───────────────────────────────────────────────────────────┤
│          │  Content (padding 16 px)                                  │
│          │                                                           │
└──────────┴───────────────────────────────────────────────────────────┘
```

- **Top bar 48 px** (увеличена с 40 px по UX-ревью: health pill + freshness не помещаются в 40 px без compromised читаемости). Содержит:
  - Слева: `pg_doorman` brand + версия (Plex Sans 14 px medium).
  - Центр-лево: HealthPill (см. 6.5) + 3-4 chips с ключевыми контаминированными счётчиками из `/api/overview`.
  - Справа: FreshnessIndicator (см. 6.6) + индикатор admin-state (`[admin]` если auth прошёл).
- **Sidebar 220 px**, текст 14 px sans, иконка 16 px lucide, padding 6 px × 12 px. Текущая страница — левая полоса 2 px `--accent` + текст medium.
- **Content padding 16 px** со всех сторон. На широких экранах (>1280 px) — `max-width: 1440px`, центрируется.
- **Sidebar collapse до 56 px** (icon-only) — follow-up, не в MVP.

Top bar — единственный «sticky» элемент (всегда виден при scroll). Page header не sticky — при длинных таблицах (clients, logs) контент важнее.

### 5.2 Сетка

8 px база (`--space-1 = 4px`, `--space-2 = 8px`, …, `--space-8 = 32px`). Пользуемся ею для всего: padding, gap, margin. Запрещены значения вне шкалы (никаких `padding: 7px`).

### 5.3 Таблицы (главный паттерн)

```
┌─────────────────────────────────────────────────┐
│ HEADER                                          │  /* surface, 13 px medium muted, 32 px height */
├─────────────────────────────────────────────────┤
│ row 1                                           │  /* 32 px height, 13 px regular */
│ row 2                                           │
│ ...                                             │
└─────────────────────────────────────────────────┘
```

- Row height 32 px. Hover — фон `--surface-2`, без анимации. Граница между строк 1 px `--border`.
- Cell padding `8px 12px`.
- Header sticky при scroll'е длинных таблиц (clients, servers, prepared, logs).
- Числовые колонки — `text-align: right`, класс `tabular`, IBM Plex Mono.
- Status-колонки — badge (см. ниже), не текст.
- Truncate длинных строк через `text-overflow: ellipsis` + `title=` для tooltip'а.

Pagination footer (для clients/servers/prepared) — 32 px, моно счётчик «1–100 of 1247» слева, кнопки «Prev / Next» справа, без номеров страниц.

### 5.4 Карточки и метрики

Только в Overview. На остальных страницах — таблицы и тонкие счётчики в page header.

Card:
- `background: var(--surface); border: 1px solid var(--border); border-radius: 4px;`
- Padding 16 px, gap между карточками 12 px.
- Заголовок карточки (label) — 11 px medium muted, caps, letter-spacing 0.05em.
- Hero metric — 28 px IBM Plex Mono semibold tabular, цвет `--text`.
- Sparkline под hero — высота 56 px, без axis labels, без grid (мини-формат).

## 6. Компоненты

Никакого UI-кита (shadcn/MUI/Chakra) — пишем 8–10 примитивов руками. Все — на CSS variables, без runtime CSS-in-JS.

### 6.1 Минимальный набор

| Компонент | Назначение | Размер |
|---|---|---|
| `Button` | Действие (Reset history, Refresh, Cancel modal) | 32 px height |
| `Badge` | Статус (active/idle/paused, error count) | 20 px height, caps 11 px |
| `HealthPill` | Глобальное OK / degraded / critical в top bar | 24 px height, caps 11 px |
| `FreshnessIndicator` | «Updated Xs ago» с цветовой индикацией возраста | 24 px height |
| `Tab` | Sub-nav на странице (Caches → Prepared/QueryCache, ConfigState разделы) | 32 px height, underline 2 px accent |
| `Input` | Поле в AuthGate modal, search box | 32 px height |
| `SearchBox` | Global search в page header (cmd-K) | 32 px height, icon prefix |
| `Modal` | AuthGate, confirm dialog | max-width 420 px, centered |
| `Drawer` | Pool detail из `/pools` row click | 480 px width, slide-in справа |
| `Banner` | «Backend unreachable», «log_tap_disabled» | 36 px height, full-width inside content |
| `Sparkline` | Mini-chart в карточках Overview / per-pool row | 56 px height (golden signals) / 24 px (per-pool inline) |
| `Heatmap` | Pool fill heatmap (Overview row 3b) | row 24 px × N pools |
| `Chart` | Полноразмерный uPlot (Overview 3a/3c/3d, ConfigState вкладки) | 200 px default |
| `LogStream` | Виртуализированный stream логов | flex-1 |
| `TimePicker` | «Jump to ts» в LogStream (follow-up MVP) | 32 px height inline |
| `Gauge` | Inline saturation gauge per-pool row | 100 px width × 16 px |
| `EmptyState` | 3 варианта: OK / Info / Warming | 120 px |
| `ThresholdPaint` (mixin) | Применяется к Sparkline / Chart / row для подсветки crit/warn состояния | — |

### 6.2 Button — варианты

```css
.btn {
  height: 32px;
  padding: 0 12px;
  font: 500 13px/1 'IBM Plex Sans';
  border-radius: 4px;
  border: 1px solid var(--border-strong);
  background: var(--surface);
  color: var(--text);
  transition: background 100ms;
}
.btn:hover    { background: var(--surface-2); }
.btn:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }

.btn-primary  { background: var(--accent); color: var(--accent-fg); border-color: transparent; }
.btn-danger   { color: var(--danger); border-color: var(--danger); }
.btn-ghost    { background: transparent; border-color: transparent; }
```

Никаких icon-buttons без `aria-label`. Никаких disabled с opacity 0.5 — лучше не показывать вовсе.

### 6.3 Badge

```
ACTIVE   IDLE   PAUSED   ERROR   TLS   PLAIN
```

11 px medium caps mono, padding 2 × 6 px, border-radius 2 px. Цвет:
- `ACTIVE` / `TLS` — `--success` фон 14% opacity, текст `--success`.
- `IDLE` / `PLAIN` — `--text-muted` без фона.
- `PAUSED` / `WARN` — `--warning` фон 14% opacity, текст `--warning`.
- `ERROR` — `--danger` фон 14% opacity, текст `--danger`.

### 6.4 LogStream

Это самый «характерный» компонент — фактически весь экран `/logs`.

**Базовый рендер:**
- Шрифт IBM Plex Mono 12 px, line-height 1.5.
- Каждая запись: `[seq] [HH:mm:ss.SSS] [LEVEL] target — message`.
- Level раскрашен: ERROR `--danger`, WARN `--warning`, INFO `--text`, DEBUG `--text-muted`, TRACE `--text-dim`.
- Target — `--accent` (как ссылка, без подчёркивания), click → добавляет в filter.
- Multi-line message: `\n` внутри message рендерится как visual line break, остальные строки идут с indent 2ch и без header'а (только сам текст). Один `LogEntry` = один логический блок, независимо от высоты в пикселях.
- Длинные строки: soft-wrap по умолчанию (160ch ≈ 1 line). Toggle «Wrap / Truncate» в header.
- Дроп: серая sticky плашка `<N> lines dropped` (фон `--surface-2`, `--text-muted` italic), отображается до тех пор, пока оператор не подтвердил «dismiss».
- Search highlight: matched substring через `<mark>` с фоном `rgba(--accent, 0.25)` без изменения цвета текста.

**Auto-scroll behaviour** (UX-ревью):
- При активном auto-scroll новая запись прокручивает к bottom.
- Когда оператор начинает scroll вверх — auto-scroll **выключается автоматически**. Sticky button «Resume tail» появляется в правом нижнем углу LogStream area.
- Click «Resume tail» → возврат к live. Также re-scroll до bottom включает обратно.

**Filter и default mode:**
- Default open mode — `level=WARN` (errors+warnings only), one-click toggle на `level=DEBUG/TRACE`. Это дешёвая защита от 80% шума при первом открытии.
- Filter передаётся в backend через `?level=&target=` (server-side, см. раздел 9.6 основной спеки), substring search — клиентский поверх уже отфильтрованного.
- Filter UI: 3 control'а в header — level select, target combobox с autocomplete по уже наблюдённым target'ам, substring input. Reset — кнопка `×` рядом.

**Time-jump (follow-up):**
- TimePicker в header «Jump to ts» (формат `HH:MM:SS.mmm`). Преобразуется в seq клиентски через бинарный поиск по уже загруженной истории. Если нужно прыгнуть за пределы загруженного — выкатывается banner «Outside loaded window, fetching…» и делается запрос с `?ts_ms=`.

**Виртуализация:** через `react-window` (FixedSizeList либо VariableSizeList для multi-line entries). Альтернатива — ручной windowing — рассматривается при имплементации фазы 6, выбор зависит от объёма зависимости.

### 6.5 HealthPill

Глобальный индикатор в top bar, всегда виден.

```
[● OK]              — фон transparent, dot --success, текст --text muted
[● degraded]        — фон rgba(--warning, 0.14), dot --warning, текст --warning
[● critical]        — фон rgba(--danger, 0.14), dot --danger, текст --danger
```

- Высота 24 px, padding `2 × 8 px`, border-radius 12 px (rounded pill).
- Текст: 11 px medium caps, letter-spacing 0.05em.
- Источник: `/api/overview.health.state`. При `state ≠ ok` справа от pill курсивом отображается `health.reason` (`--text-muted`, 11 px regular, max-width 320 px с ellipsis).
- Click → переход на `/pools?filter=critical` (или другую страницу, релевантную причине).
- Tooltip: full reason без truncate.

### 6.6 FreshnessIndicator

«Updated Xs ago» в top bar справа. Защищает оператора от ситуации «UI выглядит как live, но молча завис».

| Возраст последнего успешного poll'а | Цвет | Формат |
|---|---|---|
| < 3 s | `--text-muted` | `Updated 0.8s ago` |
| 3–10 s | `--text-muted` (норма при перерывах) | `Updated 5s ago` |
| 10–30 s | `--warning` | `Updated 14s ago — retrying` |
| > 30 s | `--danger` | `Stale 45s — backend unreachable` |

- Возраст вычисляется из `Date.now() - last_successful_poll_ts`, обновляется requestAnimationFrame'ом раз в 250 ms (без лишних re-render'ов всего дерева).
- Источник timestamp: успешный fetch любого `/api/*` endpoint'а (через `useFreshness` hook).
- Не блокирует UI — оператор продолжает видеть последнюю удачную копию данных.

### 6.7 Drawer

Slide-in справа панель для drill-down. Используется в Pools (server detail), Caches (prepared text view), Clients (client detail) — везде, где нужен «details on demand» без потери context'а.

- Width 480 px (на narrow 1120 px остаётся 640 px для основной таблицы).
- Backdrop: `rgba(0,0,0,0.5)` поверх content, click closes drawer.
- Slide-in 150 ms ease-out из `transform: translateX(100%)` в `0`.
- Header: title + close button (`X` keyboard ESC).
- Body: scrollable, padding 16 px.
- ARIA: `role=dialog aria-modal=true`, focus trap внутри.
- URL: `?drawer=<id>` — bookmarkable. F5 восстанавливает open state.

### 6.8 Heatmap

Pool fill heatmap для Overview row 3b. Каждая строка — pool, ячейки — 60 семплов saturation за последние 1.5 минуты.

- Row height 24 px, label слева 140 px (truncate ellipsis при overflow), 60 cells × 6 px каждая.
- Цвет ячейки:
  - 0–69 % saturation → `--success` opacity 0.15–0.6 (gradient по %).
  - 70–89 % → `--warning` opacity 0.4–0.8.
  - 90–100 % → `--danger` opacity 0.6–1.0.
- Hover на ячейке → tooltip с pool id, ts, saturation %, connections / max_connections.
- Click на ячейке → переход на `/pools?focus=<id>&ts=<ts>` (drill-down во временной точке).
- Click на label → переход на `/pools?focus=<id>` (без time anchor).
- При >30 пулах: первые 30 показаны, кнопка «Show all (12 hidden)» снизу разворачивает.

### 6.9 ThresholdPaint mixin

Применяется к Sparkline, Chart, table row, гауджу — единый набор visual cues для подсветки `warning` / `critical` состояния.

**Sparkline cell:**
```css
.sparkline-warn  { border-left: 2px solid var(--warning);
                   background: rgba(212, 160, 23, 0.03); }
.sparkline-crit  { border-left: 2px solid var(--danger);
                   background: rgba(229, 72, 77, 0.04); }
```

**Numeric value:** dot prefix, не цвет текста.
```html
<span class="value-warn">● 84 ms</span>   <!-- dot --warning, текст --text -->
<span class="value-crit">● 720 ms</span>  <!-- dot --danger, текст --text -->
```

**Table row** (Pools, Clients):
```css
.row-warn { border-left: 2px solid var(--warning); }
.row-crit { border-left: 2px solid var(--danger); }
```

**Chart threshold lines:** dashed horizontal через uPlot `hooks.draw`:
- Warning line: `--warning` color, dashArray `[4, 4]`, opacity 0.4.
- Critical line: `--danger` color, dashArray `[4, 4]`, opacity 0.5.
- Drawn под data line (background layer, не передний план).

**Запрещено:** flash, blink, sound, modal popup. Anomaly highlighting — пассивный visual cue, не алерт.

## 7. Графики (uPlot)

### 7.1 Общий стиль

Все графики Overview и ConfigState — uPlot, единый стиль:

- Толщина линии 1 px (не 2). Мы в industrial-стиле, не infographic.
- Цвета серий — `--chart-1..4`, в порядке появления. На stacked area — opacity 0.4 для fill, full color для top stroke.
- Ось X — Unix timestamp, формат `HH:mm:ss`. Ось Y — числа без префиксов «k/M», полная цифра в моно.
- Grid: 1 px `--border` каждые 25% оси Y, без вертикальных grid-линий.
- Axis labels: 10 px IBM Plex Mono, цвет `--text-dim`.
- Tooltip: `--surface-3` фон, 11 px mono, 1 px border `--border-strong`, padding 4 × 8 px.
- Title графика — над uPlot блоком, отдельный label, не внутри uPlot config.

### 7.2 Cross-hair sync

Все графики Overview rows 2-3 (Golden Signals strip + per-aspect detail) объединены через uPlot `sync.key = 'overview'`. Hover в любом чарте подсвечивает ту же временную точку во всех остальных. Tooltip cursor — vertical guideline 1 px `--accent`, label справа фиксированный.

`sync.key` нужен также для Pools-страницы (per-pool inline sparklines синхронизируются между собой), но **не** между разными страницами.

### 7.3 Threshold lines

Подключаются через mixin `ThresholdPaint` (раздел 6.9). Реализация — uPlot hook `draw`, рисующий dashed horizontal lines поверх grid'а перед data line. Для каждого графика, который имеет threshold (см. таблицу 15.4 в основной спеке) — обязательно показать warn и crit линии.

### 7.4 Sparkline (отдельный режим)

Mini-chart — height 56 px (Overview Golden Signals strip) либо 24 px (per-pool inline). Без axis labels, без grid (только 1 horizontal threshold line при необходимости). Tooltip по hover — числовое значение последней точки.

### 7.5 Состав графиков

См. раздел 15 «Observability layout & thresholds» основной спеки — там описано, какой тип графика на каком ряду какой страницы и какие threshold ему рисовать. Этот раздел — только про визуальный язык, не про composition.

## 8. Иконография

- Библиотека — `lucide-react`. Tree-shakable, ~3–5 KB на 20–30 нужных иконок.
- Размеры: 16 px (sidebar item, badge), 20 px (page header action), 14 px (inline в тексте).
- `stroke-width: 1.5`. Outline only, никаких filled.
- Цвет наследуется от текста. На active state — `--accent`.

Базовый набор для MVP:
`activity` (overview), `database` (pools), `users` (clients), `server` (servers), `layers` (prepared), `hash` (interner), `scroll-text` (logs), `settings` (config), `lock` (auth), `alert-triangle` (warning), `x-circle` (error), `check-circle` (ok), `pause` (paused pool), `refresh-cw` (reset), `chevron-left/right` (pagination).

## 9. Анимации

Минимум, только feedback на действия пользователя.

| Что | Длительность | Easing |
|---|---|---|
| Hover background change | 100 ms | linear |
| Focus ring появление | 0 ms (instant) | — |
| Modal in/out | 150 ms | ease-out |
| Banner slide-in | 150 ms | ease-out |
| Tab underline shift | 150 ms | ease-out |
| Skeleton pulse | 1500 ms loop | ease-in-out |

Анимировать **только** `transform` и `opacity`. Никаких `width`/`height`/`top`/`left`.

`prefers-reduced-motion: reduce` — выключаем все анимации кроме skeleton pulse (и тот делаем opacity-only, без scale).

## 10. Keyboard shortcuts

Operational tool, не SaaS-обыватель. DBA быстрее работает руками с keyboard, чем мышью. Для MVP — следующий минимальный набор (UX/DBA-ревью):

### 10.1 Global

| Shortcut | Действие |
|---|---|
| `?` | Open shortcuts overlay |
| `g o` | Перейти на Overview |
| `g p` | Перейти на Pools |
| `g c` | Перейти на Clients |
| `g s` | Перейти на Caches (Storage/Statements) |
| `g l` | Перейти на Logs |
| `g k` | Перейти на ConfigState |
| `/` | Focus search в текущей странице |
| `Esc` | Close drawer / modal / cancel filter |
| `r` | Manual refresh (форсирует следующий poll) |
| `Shift+R` | Reset history (sessionStorage) |

### 10.2 Tables

| Shortcut | Действие |
|---|---|
| `j` / `↓` | Next row |
| `k` / `↑` | Previous row |
| `Enter` | Open row drawer (если применимо) |
| `g g` | Top of table |
| `Shift+G` | Bottom of table |
| `PageDown` / `PageUp` | Scroll page |

### 10.3 LogStream

| Shortcut | Действие |
|---|---|
| `Space` | Pause / resume auto-scroll |
| `g g` | Top (oldest in ring) |
| `Shift+G` | Bottom (latest, resume tail) |
| `j` / `k` | Step один entry |
| `f` | Cycle level filter (WARN → INFO → DEBUG → TRACE → WARN) |
| `t` | Focus target filter |
| `/` | Focus substring search |
| `n` / `N` | Next / previous match (post-search) |

### 10.4 Реализация

`useKeyboard` hook в `frontend/src/hooks/`. Использует event listener на `document`, исключает срабатывание внутри `<input>` / `<textarea>` через `event.target.tagName` check. Sequence-shortcuts (`g o`, `g g`) работают через таймер: первая клавиша запускает 1500 ms окно, в котором ожидается вторая.

Невидимый focus target в каждой таблице (`tabindex={0}` на `<tbody>`) — для row navigation. Visual focus state — left rule 2 px `--accent` + slight surface-2 background.

`prefers-reduced-motion` не влияет на keyboard nav (нет анимаций, кроме scroll'а).

## 11. Состояния

### 11.1 Loading

Skeleton screens. Серый блок `--surface-2`, pulse opacity 0.5–1.0, никаких spinner'ов. Размеры повторяют будущий контент (не «общий box»).

Spinner допустим только в одном месте — кнопке во время action (например, повторного auth'а), 14 × 14 px, 1 px stroke, accent-color.

### 11.2 Empty (3 варианта)

UX-ревью: разные empty-state'ы означают разное (всё ок / informational / transitional). Конflated рендер вводит оператора в заблуждение.

**Empty-OK** — данных нет, и это норма (свежий старт, никто не подключился).
```
   [icon-circle 32px text-muted]
   No clients connected
   Connections will appear here as they arrive.
```
Серая иконка + caption, никаких CTA, никаких иллюстраций.

**Empty-Info** — operator-driven state (paused pool, отключённая фича).
```
   [icon-pause 32px warning]
   Pool main@db1 is paused
   Resume via psql admin: PAUSE main@db1 → RESUME main@db1
```
Amber иконка (`pause`/`info`), caption отвечает на немой вопрос «что с этим делать».

**Empty-Warming** — transitional, данные сейчас появятся.
```
   [skeleton row 1] (animate pulse)
   [skeleton row 2]
   [caption muted italic] Log tap activating…
```
Skeleton pulse, без иконки. После 5 секунд без обновления переключается на Empty-OK либо Error banner.

Применяется к: Pools (paused → Info), Clients (no clients → OK), Servers (warming up → Warming), LogStream (tap activating → Warming).

### 11.3 Error / disconnect

Banner вверху content area:

```
[!] Backend unreachable. Retrying every 2 seconds.
```

Цвет `--danger` border + текст, фон `--surface`. Не блокирует UI — пользователь видит последние данные. FreshnessIndicator (раздел 6.6) дополнительно показывает возраст последнего успешного poll'а.

### 11.4 Auth gate

Modal по центру, `--surface-3` фон, 1 px `--border-strong`. Заголовок «Admin authentication required», два input'а (user/password), кнопка `btn-primary` «Sign in». Под input'ами — мелким текстом «Credentials are stored in memory only».

Опциональный toggle «Remember for this tab session» (UX-ревью): при включении credentials сохраняются в `sessionStorage`, переживают F5 reload (но не browser restart). Default — выключено (memory-only).

## 12. Технические правила

1. **Tailwind 3** для верстки, но через design tokens из `:root`. То есть `tailwind.config.js` extend'ит colors/spacing/font-size **из CSS переменных**, не дублирует значения.
2. **`cn()` helper** (clsx + tailwind-merge) для условных классов.
3. **CVA** для вариантов (`button`, `badge`).
4. **Никаких dynamic class names** (`bg-${color}-500`) — только object-map'ы.
5. Все `interactive` элементы — нативные `<button>`, `<a>`, `<input>`. Никаких `<div onClick>`.
6. Focus-visible везде — без исключений.
7. Все формы — `<label>` явно, не placeholder-as-label.
8. **URL view-state** — каждая страница, у которой есть фильтр/sort/pagination, читает и пишет это в `?query` через `useUrlState` hook. F5 / share-link воспроизводят тот же view (UX/DBA-ревью, scenario H).
9. **Threshold paint** — только через mixin (раздел 6.9). Никаких inline `style={{borderColor: 'red'}}` в компонентах. Источник правды — таблица 15.4 в основной спеке.
10. **Polling и derived metrics** — компонент не делает свой `setInterval`, а использует `usePoll(endpoint, 1500)`. Derived series (qps, tps, error_rate из cumulative counter) — через `useHistory` + клиентское вычисление delta, не на backend.
11. **Cross-page event bus** — нет. Если нужно «открыть drawer на /pools при клике на cell в Heatmap» — это URL navigation (`navigate('/pools?focus=…')`), не in-memory event.

## 13. Pre-merge checklist (frontend)

Перед merge каждой страницы:
- [ ] WCAG AA: контраст всех текст/UI ≥ 4.5:1 / 3:1.
- [ ] Все `<button>` / `<a>` имеют видимый focus ring.
- [ ] Числовые колонки `tabular-nums slashed-zero`.
- [ ] Empty / error / loading states нарисованы — ВСЕ ТРИ варианта empty (OK / Info / Warming).
- [ ] Sparkline / chart axis labels есть, нет color-only smysla (паттерн / icon backup).
- [ ] Sidebar nav: текущая страница подсвечена `--accent` левой полосой.
- [ ] Модалки и dropdown закрываются по Escape.
- [ ] Test на 1120 px ширине — без horizontal scroll (laptop+slack baseline; UX-ревью).
- [ ] Test на 1440 px и >1600 px — корректное поведение `max-width: 1440px`.
- [ ] `prefers-reduced-motion: reduce` уважается.
- [ ] Threshold paint работает: при значении в crit zone — sparkline/row/value подсвечены по правилам раздела 6.9.
- [ ] Cross-hair sync на Overview — hover в любом чарте подсвечивает остальные.
- [ ] Keyboard shortcuts из раздела 10 работают на этой странице (релевантные).
- [ ] FreshnessIndicator реагирует на отключение backend (имитировать через DevTools throttling).
- [ ] HealthPill корректно отрисовывается для всех трёх state'ов.
- [ ] URL view-state: filter/sort/pagination персистятся в `?query`, F5 восстанавливает.
- [ ] Empty + URL state: при `?filter=foo` без матчей — Empty-OK, не Empty-Info.

## 14. Что осталось решить позднее

- Точный набор иконок lucide для каждой страницы — фиксируется при имплементации фазы 6.
- Конкретные snapshot'ы JSON для unit-тестов уже описаны в основной спеке — здесь только визуал.
- Light theme — после MVP.
- Mobile / narrow layout — после MVP.
- Конкретный механизм виртуализации `LogStream` — выбрать `react-window` vs ручной windowing на этапе фазы 6.
- Custom font subset (брать ли только latin или включать cyrillic) — при первом замере итогового размера бандла.
- Дизайн `?` shortcut overlay (компактная grid из 10.1-10.3) — после фазы 6 на основе реального размера shortcut-таблицы.
- `Ctrl+K` quick command palette — если keyboard-power-user в реальной эксплуатации останется недоволен sequence shortcut'ами `g <letter>`.
- Анимация переходов между страницами (slide / fade) — если по результатам user-testing будет ощущение разрыва. По умолчанию — instant nav, как в Linear.
- TimePicker UX в LogStream — если 8 KB cap (8192 entries) хватает на сценарий «jump to 14:32», то picker — niceness; если нет — must-fix в 3.8.1.
- Mobile / narrow layout — после MVP.
- Конкретный механизм виртуализации `LogStream` — выбрать `react-window` vs ручной windowing на этапе фазы 6.
- Custom font subset (брать ли только latin или включать cyrillic) — при первом замере итогового размера бандла.

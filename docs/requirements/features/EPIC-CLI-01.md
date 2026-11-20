# EPIC-CLI-01: 인간 친화적 CLI 사용성 및 다중 에이전트 하네스 프로비저닝 (Human-Friendly CLI UX & Multi-Harness Provisioning)

## 1. Introduction
### 1.1 Purpose
본 스펙은 인간 사용자(Human User)가 터미널에서 `omk CLI`를 실행할 때 겪는 사용성 결함(무응답 대기 현상, 비직관적인 Raw JSON 출력, 상태 확인 수단 부재, 인자 오타 무시 현상)을 해결하고, 다양한 AI 에이전트 하네스(AI Agent Harness) 환경을 자동으로 탐지하거나 명시적으로 지정하여 `omk Skill` 및 MCP 서버 설정을 안전하게 프로비저닝(Provisioning)하는 표준 동작을 정의합니다.

### 1.2 Scope
- 터미널(TTY) 인터랙티브 실행 감지 및 기본 진입 동작(Help / Status 안내 vs Session Proxy) 제어.
- 인간 친화적(Human-Readable) 포맷 출력과 머신/스크립트 친화적(`--json`) 출력 모드의 이원화.
- 명령줄 인자(Subcommand, Flag) 엄격 검증 및 친절한 오류/가이드 메시지 제공.
- 시스템, Vault Server(`omk daemon`), 로컬 금고(Vault), 하네스 설치 현황을 진단하는 `omk status` 명령어.
- 주요 AI 에이전트 하네스(Claude Code, Claude Desktop, Cursor, Antigravity/Gemini CLI, Codex, OpenCode, Windsurf) 자동 탐지(Auto-detection) 및 `--harness` 타겟 프로비저닝 옵션.
- 기존 하네스 설정 파일의 비파괴적 병합(Non-destructive Config Merge) 보장.

### 1.3 Definitions, Acronyms, and Abbreviations
- **AI Agent Harness (하네스):** LLM 에이전트를 구동하며 외부 도구(MCP Server) 및 스킬 지침(`SKILL.md`, 시스템 프롬프트 등)을 마운트할 수 있는 클라이언트 애플리케이션 또는 CLI 환경(예: Claude Code, Cursor, Antigravity 등).
- **TTY (TeleTYpewriter):** 표준 입출력(`stdin`/`stdout`)이 파이프나 서브프로세스 스트림이 아닌 사용자의 인터랙티브 터미널 콘솔에 직접 연결된 상태.
- **Non-destructive Merge (비파괴적 병합):** 하네스의 기존 설정 파일(예: `mcp_config.json`, `claude_desktop_config.json`)에 등록된 타 MCP 서버나 사용자 설정을 훼손하지 않고 `mcpServers.omk` 엔트리만 안전하게 삽입하거나 갱신하는 동작.
- 기타 핵심 도메인 용어(`omk CLI`, `omk Skill`, `Local Vault` 등)는 [용어 사전 (Glossary)](../glossary.md)을 따릅니다.

### 1.4 References
- [`CONSTITUTION.md`](../../../CONSTITUTION.md) : Agent-Agnostic(에이전트 범용성) 및 Environment Isolation(환경 독립성) 원칙
- [`docs/requirements/glossary.md`](../glossary.md) : 프로젝트 용어 사전 및 도메인 경계
- [`docs/requirements/features/EPIC-IPC-01.md`](EPIC-IPC-01.md) : MCP 통신 프로토콜 및 Vault Server & Session Proxy 생명주기

---

## 2. Overall Description
### 2.1 Product Perspective & Current UX Defect Analysis (현행 불친절 요소 분석)
현재 `omk CLI`(`skill/src/main.rs` 및 `skill/src/agent_prompt/mod.rs`)는 에이전트 간 IPC 통신 중심으로 구현되어 있어, 시스템을 최초 설치하고 관리해야 하는 인간 사용자(Human User) 관점에서 다음과 같은 심각한 사용성 결함(Usability Defects)을 내포하고 있습니다.

| 결함 ID | 현행 동작 (`skill/src/main.rs`, `agent_prompt/mod.rs`) | 인간 사용자(Human User) 관점의 문제점 | 해결 요구사항 |
| :--- | :--- | :--- | :--- |
| **DEF-CLI-01** | 인자 없이 `omk` 실행 시 무조건 `mcp` 서브커맨드로 폴백하여 `stdin.read_line()` 무한 루프 진입 | 사용자가 터미널에서 `omk`를 입력하면 아무런 출력 없이 프로세스가 멈춘(Hang) 것처럼 보여 당황하게 됨. | `REQ-CLI-001` |
| **DEF-CLI-02** | `help`, `--help`, `-h` 실행 시 `{"name":"omk","version":"0.1.0","commands":[...]}` Raw JSON 1줄만 출력 | 각 명령어의 역할, 사용 가능한 옵션(`--vault`, `--harness`, `--target`), 사용 예시를 전혀 알 수 없음. | `REQ-CLI-001`<br>`REQ-CLI-002` |
| **DEF-CLI-03** | `install`, `reconcile`, `stop` 및 에러 발생 시 한 줄짜리 Raw JSON 또는 JSON-RPC 에러 객체(`{"jsonrpc":"2.0","error":...}`)만 출력 | 인간 사용자가 명령의 성공 여부, 변경된 파일 경로, 실패 원인 및 조치 방법을 직관적으로 파악하기 어려움. | `REQ-CLI-002` |
| **DEF-CLI-04** | 알 수 없는 서브커맨드(예: `omk status`, `omk instal`)나 잘못된 플래그 입력 시 무시하고 넘어감 (오타 플래그는 무시된 채 기본 `mcp` 루프로 빠져 멈춤) | 사용자의 타이핑 실수를 피드백하지 않고 오동작(무한 대기)을 유발함. | `REQ-CLI-001` |
| **DEF-CLI-05** | 현재 데몬 구동 여부, Vault 경로 및 인덱싱된 문서 수, 연결된 하네스 상태를 확인하는 명령어 부재 | 시스템이 정상 작동 중인지 확인하려면 사용자가 직접 소켓 파일이나 DB, 프로세스 목록(`ps`)을 뒤져야 함. | `REQ-CLI-003` |
| **DEF-CLI-06** | `omk install`이 `~/.gemini` 경로에만 하드코딩되어 있으며, 기존 `mcp_config.json`에 타 서버가 있으면 덮어쓰거나 설치를 건너뜀 | `CONSTITUTION.md`의 **Agent-Agnostic** 원칙 위배. Claude, Cursor, Codex 등 대중적인 하네스 사용자가 수동으로 JSON을 편집해야 하며 기존 설정 유실 위험이 있음. | `REQ-CLI-004`<br>`REQ-CLI-005` |

### 2.2 User Classes and Characteristics
- **Human User (인간 사용자 / 개발자):** 터미널에서 `omk install`, `omk status`, `omk reconcile`, `omk stop` 등을 실행하여 로컬 지식 금고와 AI 에이전트 하네스 환경을 설정하고 점검하는 주체. 직관적이고 구조화된 텍스트 피드백을 기대합니다.
- **AI Agent Harness / Automation Script:** `omk mcp`를 서브프로세스로 기동하거나 `--json` 플래그를 통해 CLI 출력을 파싱하는 비대화형(Non-interactive) 주체. 표준을 준수하는 결정론적 JSON/JSON-RPC 출력을 기대합니다.

### 2.3 Design and Implementation Constraints
- **Zero Heavy External Dependencies (`CONSTITUTION.md` §2):** CLI 인자 파싱, TTY 감지, JSON 설정 병합은 외부 대형 크레이트(예: `clap`, `serde_json` 등)를 임의로 추가하지 않고, Rust 표준 라이브러리(`std`)와 `omk_core` 내장 파서(`JsonValue`) 및 최소한의 OS/POSIX 인터페이스(`isatty` 등)를 활용하여 정적 단일 바이너리의 경량성을 유지해야 합니다.

---

## 3. System Features

### 3.1 REQ-CLI-001: 지능형 CLI 진입점, 엄격한 인자 검증 및 구조화된 도움말 (Entry Point, Strict Parsing & Help UX)
- **3.1.1 Description and Priority:** 매우 높은 우선순위 (핵심 사용성). 인자 없는 실행 시의 멈춤 현상을 방지하고, 명확한 사용법 가이드와 오타 검증을 제공합니다.
- **3.1.2 Stimulus/Response Sequences:**
  - *Stimulus 1:* 사용자가 인터랙티브 터미널(TTY)에서 인자 없이 `omk`를 실행함.
  - *Response 1:* `omk CLI`는 Stdio 루프로 진입하여 멈추지 않고, 즉시 요약 상태 및 구조화된 도움말(Help)을 출력한 뒤 종료 코드 `0`으로 종료함. (단, `stdin`이 파이프/서브프로세스인 Non-TTY 환경에서 인자 없이 호출된 경우 하위 호환성을 위해 `omk mcp`로 동작하되, 표준 권장 설정은 명시적 `omk mcp` 호출임.)
  - *Stimulus 2:* 사용자가 `omk --help`, `omk -h`, `omk help [subcommand]` 또는 `--version` / `-V`를 실행함.
  - *Response 2:* 버전 정보, 명령어 목록, 플래그 설명, 주요 사용 예시가 정렬된 텍스트를 출력하고 종료 코드 `0`으로 종료함.
  - *Stimulus 3:* 사용자가 정의되지 않은 서브커맨드(예: `omk instal`)나 알 수 없는 플래그(예: `omk install --unknown`), 또는 필수 인자가 누락된 플래그(예: `omk install --harness`)를 입력함.
  - *Response 3:* 무시하거나 `mcp`로 폴백하지 않고, 즉시 잘못된 인자를 지적하는 에러 메시지와 유사 명령어 제안 또는 `omk --help` 안내를 `stderr`에 출력한 뒤 비정상 종료 코드(`2` 또는 `1`)로 종료함.
- **3.1.3 Functional Requirements:**
  - 시스템은 `stdin`이 인터랙티브 터미널(TTY)인지 판별해야 하며, TTY 환경에서 서브커맨드 없이 `omk`가 단독 실행될 경우 블로킹 대기 없이 도움말 및 빠른 시작 가이드(`omk install`, `omk status` 안내)를 출력해야 한다.
  - 명시적으로 `omk mcp`가 호출된 경우, 만약 `stdin`이 TTY라면 `stderr`에 `"[omk] Starting MCP Session Proxy (waiting for JSON-RPC input on stdin... Press Ctrl+C to exit)"` 안내 문구를 1회 출력하여 사용자가 프로세스 멈춤으로 오인하지 않도록 해야 한다 (`stdout`은 프로토콜 보호를 위해 오염시키지 않는다).
  - CLI 파서는 다음 서브커맨드와 전역/지역 플래그를 명확히 식별하고, 정의되지 않은 위치 인자나 플래그 입력 시 즉시 오류를 반환해야 한다:
    - 서브커맨드: `install`, `status`, `reconcile`, `stop`, `mcp`, `daemon`, `help`
    - 플래그: `--vault <dir>` (`-v`), `--harness <names>` (`-H`), `--target <dir>` (`-t`), `--json`, `--version` (`-V`), `--help` (`-h`)
  - `omk help` (또는 `--help`, `-h`) 출력에는 다음 항목이 반드시 포함되어야 한다:
    1. 애플리케이션 이름, 버전, 한 줄 설명 (`omk (oh my knowledge) - Standalone Local Knowledge Vault`)
    2. `USAGE:` 구문
    3. `COMMANDS:` 각 서브커맨드(`install`, `status`, `reconcile`, `stop`, `mcp`, `daemon`, `help`)의 역할 설명
    4. `OPTIONS:` `--vault`, `--harness`, `--target`, `--json`, `--help`, `--version` 설명 및 기본값
    5. `EXAMPLES:` 일반적인 설치(`omk install`), 특정 하네스 지정 설치(`omk install --harness claude-code,cursor`), 상태 확인(`omk status`) 예시

### 3.2 REQ-CLI-002: 인간 친화적 출력 포맷 및 `--json` 모드 분리 (Human-Friendly Output vs Machine JSON Mode)
- **3.2.1 Description and Priority:** 높은 우선순위. 인간 사용자에게는 가독성 높은 상태 요약과 가이드를 제공하고, 자동화 도구에는 기존과 동일한 결정론적 JSON 출력을 보장합니다.
- **3.2.2 Stimulus/Response Sequences:**
  - *Stimulus 1:* 사용자가 터미널에서 `omk install`, `omk reconcile`, `omk stop`을 기본 모드로 실행함.
  - *Response 1:* 각 작업의 수행 결과(프로비저닝된 하네스 목록 및 경로, 스캔/재인덱싱된 문서 수, 데몬 종료 상태)를 가독성 높은 멀티라인 텍스트로 출력함.
  - *Stimulus 2:* 사용자 또는 스크립트가 `--json` 플래그를 포함하여 실행함(예: `omk reconcile --json`).
  - *Response 2:* 단일 라인 구조화 JSON 객체를 `stdout`에 출력함.
- **3.2.3 Functional Requirements:**
  - `omk mcp`를 제외한 모든 관리 명령어(`install`, `status`, `reconcile`, `stop`, `help`)는 기본적으로 **Human-Friendly 텍스트 포맷**으로 결과를 출력해야 한다.
  - `--json` 플래그가 지정된 경우, 모든 관리 명령어는 부가적인 장식 텍스트 없이 유효한 단일 JSON 문자열만 `stdout`에 출력해야 한다 (기존 테스트 및 자동화 스크립트 하위 호환성 유지).
  - 일반 관리 명령어 실행 중 에러가 발생할 경우:
    - 기본 모드에서는 JSON-RPC 프레임(`{"jsonrpc":"2.0","error":...}`)을 노출하지 않고, `[ERROR] <원인 설명>` 및 해결을 위한 `[HINT] <조치 방법>`을 `stderr`에 인간 친화적인 문장으로 출력해야 한다.
    - `--json` 모드 또는 `omk mcp` 실행 중에는 기존 표준 JSON / JSON-RPC 2.0 에러 포맷(`EPIC-IPC-01` `REQ-IPC-003`)을 유지해야 한다.

### 3.3 REQ-CLI-003: 시스템 및 하네스 상태 진단 명령어 (`omk status`)
- **3.3.1 Description and Priority:** 높은 우선순위. 사용자가 로컬 금고, Vault Server(`omk daemon`), 그리고 각 AI 에이전트 하네스의 연동 상태를 한눈에 진단할 수 있도록 합니다.
- **3.3.2 Stimulus/Response Sequences:**
  - *Stimulus:* 사용자가 `omk status [--vault <dir>] [--json]`를 실행함.
  - *Response:* Vault 경로 및 문서/인덱스 현황, Vault Server 실행 상태(PID, 소켓 응답 여부), 로컬에 감지된 AI 하네스별 프로비저닝 상태를 출력함.
- **3.3.3 Functional Requirements:**
  - `omk status`는 Vault Server를 강제로 자동 기동(Auto-spawn)하지 않고 현재 상태를 비파괴적(Read-only)으로 점검해야 한다:
    1. **Vault 상태:** 해석된 Vault 디렉터리 절대 경로, 디렉터리 존재 여부, 저장된 마크다운(`.md`) 문서 수, SQLite 인덱스 DB(`index.db`) 존재 여부 및 인덱싱된 문서 수.
    2. **Vault Server (`omk daemon`) 상태:** `daemon.pid` 파일 및 `flock` / Unix Domain Socket(`daemon.sock`) 연결 및 `ping` 응답 여부를 확인하여 `Running (PID: <pid>, Socket: <path>)` 또는 `Stopped` 상태를 표시해야 한다.
    3. **Harness 상태:** 지원 대상 유명 하네스(`REQ-CLI-004`) 각각에 대해 로컬 감지 여부(`Detected` / `Not Found`)와 `omk` MCP/Skill 설정 등록 여부(`Configured` / `Not Configured`)를 표 또는 리스트 형태로 출력해야 한다.
  - `--json` 플래그 사용 시 위 진단 정보를 구조화된 JSON(`{"vault":{...},"daemon":{...},"harnesses":[...]}`)으로 반환해야 한다.

### 3.4 REQ-CLI-004: 유명 AI 에이전트 하네스 자동 인식 (Multi-Harness Auto-Detection)
- **3.4.1 Description and Priority:** 매우 높은 우선순위 (`CONSTITUTION.md` §1 Agent-Agnostic 원칙 구현). 사용자의 로컬 환경에 설치된 유명 AI 에이전트 하네스를 자동으로 탐지하고 각각의 규격에 맞게 MCP 서버와 `omk Skill`을 프로비저닝합니다.
- **3.4.2 Stimulus/Response Sequences:**
  - *Stimulus:* 사용자가 별도의 타겟 옵션 없이 `omk install`을 실행함.
  - *Response:* 시스템이 홈 디렉터리 및 OS 표준 설정 경로를 스캔하여 설치된 모든 지원 하네스를 자동 감지하고, 감지된 하네스들에 대해 MCP 설정 병합 및 `SKILL.md` 프로비저닝을 수행한 후 결과 리포트를 출력함.
- **3.4.3 Functional Requirements:**
  - 시스템은 다음 **7대 주요 AI 에이전트 하네스(Well-Known Harnesses)**의 로컬 환경을 식별자(Canonical Slug)와 탐지 기준(Detection Rule)에 따라 인식해야 한다:

| 하네스 식별자 (`--harness`) | 대상 제품명 | 로컬 설치 탐지 기준 (Detection Condition) | MCP 설정 파일 경로 (MCP Config Target) | 스킬 지침 경로 (Skill / Rule Target) |
| :--- | :--- | :--- | :--- | :--- |
| **`claude-code`** | Anthropic Claude Code CLI | `~/.claude` 디렉터리 또는 `~/.claude.json` 파일 존재 (또는 `PATH` 내 `claude` 바이너리 존재) | `~/.claude.json` (`mcpServers.omk`) | `~/.claude/skills/omk/SKILL.md` |
| **`claude-desktop`** | Anthropic Claude Desktop | OS별 Claude 앱 설정 디렉터리 존재:<br>- macOS: `~/Library/Application Support/Claude`<br>- Linux: `~/.config/Claude`<br>- Windows: `%APPDATA%\Claude` | `<AppSupport>/Claude/claude_desktop_config.json` (`mcpServers.omk`) | N/A (공용 `~/.omk/skills/omk/SKILL.md` 생성) |
| **`cursor`** | Cursor IDE | `~/.cursor` 디렉터리 존재 | `~/.cursor/mcp.json` (`mcpServers.omk`) | `~/.cursor/skills/omk/SKILL.md` |
| **`gemini`**<br>*(별칭: `antigravity`, `agy`)* | Google Antigravity / Gemini CLI | `~/.gemini` 디렉터리 존재 | `~/.gemini/config/mcp_config.json` 및 `~/.gemini/settings.json` (`mcpServers.omk`) | `~/.gemini/skills/omk/SKILL.md` |
| **`codex`** | OpenAI Codex CLI | `~/.codex` 디렉터리 존재 (또는 `PATH` 내 `codex` 바이너리 존재) | `~/.codex/mcp_config.json` (및 `~/.codex/config.toml` 안내/스니펫) | `~/.codex/skills/omk/SKILL.md` |
| **`opencode`** | OpenCode CLI | `~/.config/opencode` 또는 `~/.opencode` 디렉터리 존재 | `~/.config/opencode/opencode.json` (또는 `~/.opencode/mcp.json`) | `~/.config/opencode/skills/omk/SKILL.md` |
| **`windsurf`** | Codeium Windsurf IDE | `~/.codeium/windsurf` 디렉터리 존재 | `~/.codeium/windsurf/mcp_config.json` (`mcpServers.omk`) | `~/.codeium/windsurf/skills/omk/SKILL.md` |

  - **비파괴적 설정 병합 (Non-destructive JSON Merge):**
    - 대상 하네스의 MCP JSON 설정 파일이 이미 존재할 경우, 파일 전체를 덮어써서 기존의 다른 MCP 서버 설정(예: `github`, `filesystem` 등)이나 상위 설정 키를 유실시켜서는 안 된다.
    - 기존 JSON 객체를 파싱한 후 최상위 `"mcpServers"`(또는 하네스별 대응 키) 객체 내부의 `"omk"` 키만 추가/갱신하여 저장해야 한다.
    - 만약 기존 설정 파일이 문법적으로 손상(Malformed JSON)되어 파싱할 수 없는 경우, 임의로 덮어쓰지 않고 `.bak` 백업 파일을 생성하거나 경고를 출력하고 해당 파일 수정을 건너뛰어 사용자 데이터를 보호해야 한다.
  - **폴백(Fallback) 보장:**
    - 자동 탐지(`auto`) 실행 시 로컬 환경에서 위 유명 하네스가 단 하나도 감지되지 않더라도 실패하지 않고, 기본 공용 경로(`~/.omk/skills/omk/SKILL.md`, `~/.omk/mcp_config.json`) 및 기본 호환 경로(`~/.gemini/skills/omk/SKILL.md`)에 프로비저닝을 수행한 뒤, 추후 원하는 하네스에 연결할 수 있도록 `--harness <name>` 사용법을 안내해야 한다.

### 3.5 REQ-CLI-005: 하네스 타겟 지정 옵션 (`--harness` / `--target`)
- **3.5.1 Description and Priority:** 높은 우선순위. 사용자가 자동 탐지 대신 특정 하네스에만 설치하거나, 아직 실행 전인 하네스 환경을 미리 강제 프로비저닝하거나, 커스텀 디렉터리를 지정할 수 있도록 제어권을 부여합니다.
- **3.5.2 Stimulus/Response Sequences:**
  - *Stimulus 1:* 사용자가 `omk install --harness cursor` 또는 복수 지정 `omk install --harness claude-code,cursor` (또는 `--harness claude-code --harness cursor`)를 실행함.
  - *Response 1:* 로컬에 해당 하네스 디렉터리가 아직 없더라도 필요한 상위 디렉터리를 생성하여 지정된 하네스(들)에만 `omk Skill` 및 MCP 설정을 프로비저닝함.
  - *Stimulus 2:* 사용자가 `omk install --harness all`을 실행함.
  - *Response 2:* 지원되는 모든 유명 하네스(OS 호환 대상 전체)에 대해 일괄 프로비저닝을 수행함.
  - *Stimulus 3:* 사용자가 지원하지 않는 하네스 이름(예: `omk install --harness unknown-ide`)을 입력함.
  - *Response 3:* 즉시 오류를 반환하고 지원 가능한 하네스 식별자 목록(`auto`, `all`, `claude-code`, `claude-desktop`, `cursor`, `gemini`, `antigravity`, `codex`, `opencode`, `windsurf`)을 안내함.
  - *Stimulus 4:* 사용자가 `omk install --target /custom/path`를 실행함.
  - *Response 4:* 지정된 `/custom/path` 디렉터리 내부에 `SKILL.md`와 `mcp_config.json`을 생성함 (`--harness`와 함께 사용될 경우 두 타겟 모두 처리하거나 명확한 우선순위로 동작).
- **3.5.3 Functional Requirements:**
  - `omk install`은 `--harness <target>` (단축 플래그 `-H <target>`) 옵션을 지원해야 한다.
  - `<target>` 값으로는 다음을 지원해야 한다:
    - `auto` (기본값): `--harness`와 `--target`이 모두 생략된 경우 동작하며, 로컬에 존재하는 하네스를 자동 감지하여 설치한다.
    - `all`: 감지 여부와 무관하게 현재 OS에서 지원 가능한 모든 유명 하네스의 설정 경로를 생성 및 프로비저닝한다.
    - 개별 하네스 식별자 및 쉼표(`,`) 구분 목록: `claude-code`, `claude-desktop`, `cursor`, `gemini` (별칭 `antigravity`, `agy`), `codex`, `opencode`, `windsurf`. 대소문자를 구분하지 않고(Case-insensitive) 매칭해야 한다.
  - 사용자가 특정 하네스를 명시적으로 지정(`--harness <name>`)한 경우, 해당 하네스의 기본 디렉터리가 아직 존재하지 않아도 디렉터리를 생성(`mkdir -p`)하여 프로비저닝을 완수해야 한다.
  - 기존 `--target <dir>` (`-t <dir>`) 옵션은 커스텀 경로에 `SKILL.md`와 `mcp_config.json`을 직접 출력하는 용도로 완전한 하위 호환성을 유지해야 하며, `--target`만 단독 지정된 경우 사용자 홈의 하네스 설정 파일들을 임의로 변경하지 않아야 한다(격리된 테스트 및 커스텀 배포 보장).

---

## 4. Quality Attributes (Non-Functional Requirements)

### 4.1 Usability (사용성)
- **Zero-Hang Guarantee:** 인터랙티브 터미널(TTY)에서 사용자가 어떤 명령어나 오타를 입력하더라도 아무런 안내 메시지 없이 프로세스가 블로킹(Hang)되는 현상이 발생하지 않아야 한다 (`omk mcp` 명시 호출 시에도 `stderr` 안내 출력).
- **Self-Documenting Errors:** 모든 CLI 인자 오류 및 실행 실패 메시지는 원인과 함께 사용자가 즉시 복사하여 실행할 수 있는 올바른 명령어 예시(Actionable Hint)를 포함해야 한다.

### 4.2 Reliability & Safety (신뢰성 및 데이터 안전성)
- **Config Idempotency (멱등성):** `omk install`을 동일한 환경에서 여러 번 반복 실행하더라도 하네스 설정 파일 내 `omk` 엔트리가 중복 생성되거나 파일 포맷이 깨지지 않고 항상 동일한 최신 상태를 유지해야 한다.
- **Zero Collateral Damage:** 타사 MCP 서버 설정이 존재하는 JSON 설정 파일을 수정할 때 기존 설정 키와 배열/객체 구조가 100% 보존되어야 한다.

---

## 5. Traceability Matrix (추적성 매트릭스)

| Requirement ID | Requirement Title | Target Module / File | Verification Method |
| :--- | :--- | :--- | :--- |
| **`REQ-CLI-001`** | 지능형 CLI 진입점, 엄격한 인자 검증 및 구조화된 도움말 | `skill/src/main.rs` | Unit & Integration Test (TTY/Non-TTY 분기, 오타 플래그 종료 코드 검증, `help` 출력 검증) |
| **`REQ-CLI-002`** | 인간 친화적 출력 포맷 및 `--json` 모드 분리 | `skill/src/main.rs` | Integration Test (기본 텍스트 포맷 vs `--json` 단일 라인 JSON 파싱 검증) |
| **`REQ-CLI-003`** | 시스템 및 하네스 상태 진단 명령어 (`omk status`) | `skill/src/main.rs`, `skill/src/daemon/mod.rs` | Integration Test (데몬 구동 전/후 `status` 및 `status --json` 출력 검증) |
| **`REQ-CLI-004`** | 유명 AI 에이전트 하네스 자동 인식 및 비파괴적 병합 | `skill/src/agent_prompt/mod.rs` | Unit & Integration Test (가상 HOME 디렉터리 내 하네스 감지 및 기존 JSON 보존 병합 테스트) |
| **`REQ-CLI-005`** | 하네스 타겟 지정 옵션 (`--harness` / `--target`) | `skill/src/main.rs`, `skill/src/agent_prompt/mod.rs` | Unit & Integration Test (`--harness cursor,claude-code`, `--harness all`, `--target` 격리 검증) |

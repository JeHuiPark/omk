# omk (oh my knowledge)

[English](README.md) | **한국어**

AI 에이전트를 위한 독립형 로컬 지식 금고(Standalone Local Knowledge Vault) 스킬 및 MCP 데몬/프록시입니다.

## 개요 (Overview)

`omk`는 AI 에이전트가 대화 세션이 바뀌더라도 사용자의 로컬 환경에서 지식을 복리로 축적하고, 과거의 지식을 현재 맥락에 선제적으로 연결(**Proactive Grounding**)할 수 있도록 지원합니다. 외부 서드파티 크레이트 의존성 없이 순수 Rust 표준 라이브러리만으로 빌드되는 단일 정적 바이너리로 동작하며, 사람이 직접 읽고 편집할 수 있는 **Markdown 파일(단일 진실 공급원, SSOT)**과 초고속 **SQLite FTS5 검색 인덱스**를 결합했습니다.

## 빠른 시작 (Quick Start)

```bash
# 1. 단일 정적 바이너리 빌드
make build

# 2. 설치된 AI 에이전트 하네스 자동 감지 및 SKILL.md / MCP 설정 프로비저닝
./target/release/omk install

# 또는 특정 하네스만 명시적으로 지정하여 설치:
# (claude-code, claude-desktop, cursor, gemini/antigravity, codex, opencode, windsurf, all)
./target/release/omk install --harness claude-code,cursor

# 3. 로컬 금고(Vault), 백그라운드 데몬 및 하네스 연동 상태 진단
./target/release/omk status
./target/release/omk stats
./target/release/omk bench --docs 3000 --turns 80
```

### 일상적인 사용 방식 (How It Works)

1. **최초 1회 프로비저닝 (`omk install`):** 로컬에 설치된 주요 AI 에이전트 하네스를 자동으로 감지하고, 기존 MCP 설정을 훼손하지 않으면서(Non-destructive Merge) `omk` 서버와 `SKILL.md` 지침을 등록합니다.
2. **수동 명령어 호출 불필요 (Zero Manual CLI Calls):** 설치가 끝나면 지식을 저장하거나 찾기 위해 사용자가 직접 CLI 명령어를 입력할 필요가 없습니다. 평소처럼 AI 에이전트와 자연어로 대화하면, 에이전트가 스스로 로컬 금고(`~/.omk/vault`, `OMK_VAULT_DIR` 또는 `--vault <dir>`로 변경 가능)를 검색하고 새로운 의사결정을 축적합니다.
3. **상태 점검 및 유지보수:** 언제든 `omk status`로 연동 상태를 확인하거나, `omk stats`로 개인정보 노출 없는 구간별 지연 시간(P50/P95) 및 검색 품질 지표를 분석하고, `omk bench`로 8대 정적 복잡도 불변식(`MET-STATIC-01..08`) 및 가상 워크로드 SLA를 검증하며, 마크다운 파일을 직접 수정한 뒤 `omk reconcile`로 인덱스를 동기화할 수 있습니다. 전체 CLI 옵션은 `omk --help` 또는 [`EPIC-CLI-01`](docs/requirements/features/EPIC-CLI-01.md), [`EPIC-OBS-01`](docs/requirements/features/EPIC-OBS-01.md), [`EPIC-PERF-01`](docs/requirements/features/EPIC-PERF-01.md) 명세를 참고하십시오.

## 문서 및 지식 맵 (Documentation)

시스템 아키텍처, 프로토콜 규격 및 세부 요구사항 명세는 단일 진실 공급원(SSOT) 원칙에 따라 [`AGENTS.md`](AGENTS.md)에 등재된 아래 문서들을 참조하십시오:

- [`CONSTITUTION.md`](CONSTITUTION.md): 프로젝트 헌법, 핵심 설계 원칙 및 거버넌스
- [`docs/architecture.md`](docs/architecture.md): 데몬-프록시 IPC, CQRS 파이프라인 및 스토리지 격리 아키텍처
- [`docs/protocol_spec.json`](docs/protocol_spec.json): MCP 및 JSON-RPC 2.0 도구 스키마 및 에러 규격
- [`docs/requirements/index.md`](docs/requirements/index.md): 비즈니스 비전 및 기능별 상세 요구사항(`EPIC-*`) 진입점

## 🙏 감사의 말 (Acknowledgments)

이 프로젝트는 [Andrej Karpathy의 LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) 아이디어와 개념에서 깊은 영감을 받았습니다.

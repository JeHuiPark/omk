# omk (oh my knowledge)

[English](README.md) | **한국어**

AI 에이전트를 위한 독립형 로컬 지식 금고(Standalone Local Knowledge Vault) 스킬 및 MCP 데몬/프록시입니다.

## 개요 (Overview)

`omk`는 AI 에이전트가 대화 세션이 바뀌더라도 사용자의 로컬 환경에서 지식을 복리로 축적하고, 과거의 지식을 현재 맥락에 선제적으로 연결(**Proactive Grounding**)할 수 있도록 지원합니다.
외부 서드파티 크레이트 의존성 없이 순수 Rust 표준 라이브러리와 시스템 C ABI(`libsqlite3`, `flock`)만으로 구현된 단일 정적 바이너리로 동작하며, 사람이 직접 읽고 편집할 수 있는 **Markdown 파일(단일 진실 공급원, SSOT)**과 초고속 **SQLite FTS5 전문 검색 인덱스**를 결합했습니다.

## 핵심 아키텍처 (Architecture Highlights)

- **글로벌 싱글톤 데몬 & Stdio 프록시 ([`EPIC-IPC-01`](docs/requirements/features/EPIC-IPC-01.md)):**
  - `omk daemon`: OS 파일 락(`flock`, `daemon.pid`)을 통해 시스템당 단 하나의 프로세스만 백그라운드에서 상주하며, Unix Domain Socket을 통해 모든 파일 쓰기와 DB 조회를 통제하고 큐 포화 시 백프레셔(`-32001`)로 방어합니다.
  - `omk mcp`: AI 에이전트 세션마다 실행되는 경량 Stdio 프록시입니다. 데몬이 꺼져 있으면 자동으로 백그라운드 기동(Auto-spawn)하고, 지수적 백오프(Exponential Backoff) 재시도와 명시적 타임아웃(`-32000`)을 통해 안전하게 요청을 전달합니다.
- **CQRS 파이프라인 및 스토리지 계층 격리 ([`EPIC-VAULT-01`](docs/requirements/features/EPIC-VAULT-01.md), [`EPIC-SEARCH-01`](docs/requirements/features/EPIC-SEARCH-01.md)):**
  - `DocumentStore` (`LocalMarkdownDocumentStore`): 임시 파일 작성 후 원자적 교체(Atomic Rename)를 통한 안전한 Markdown + YAML Frontmatter 저장 및 Path Traversal 차단(`-32003`).
  - `IndexStore` (`SqliteFtsIndexStore`): SQLite WAL 모드 기반의 블로킹 없는 병렬 읽기, FTS5 전문 검색(`AND`, `OR`), JSON 메타데이터 필터링 지원.
  - 구동 시 자가 복구(`reconcile`): 데몬 기동 시 마크다운 파일의 `version_hash`와 DB 인덱스를 대조하여 누락되거나 직접 수정된 문서를 자동으로 재인덱싱합니다.
- **출처(Provenance) 권위 보호 및 동기화 전략 ([`EPIC-DATA-01`](docs/requirements/features/EPIC-DATA-01.md)):**
  - 지식 출처의 권위(`USER_EXPLICIT` 1.0, `EXTERNAL_SYNC` 0.9, `AGENT_INFERRED` 0.6)를 구분하여, AI가 추론한 지식이 사용자가 명시한 지식을 임의로 덮어쓰지 못하도록 원천 차단(`-32002`)합니다.
  - 데이터 특성에 맞춘 4가지 동기화 전략(`UPSERT_FULL`, `APPEND_ONLY`, `STATE_PATCH`, `LINK_REFERENCE`)을 지원합니다.

## 빠른 시작 (Quick Start)

```bash
# 1. 릴리스 바이너리 빌드
make build

# 2. 단위 및 통합 테스트 실행
make test

# 3. 원클릭 스킬 및 글로벌 MCP 설정 프로비저닝
./target/release/omk install
```

최초 1회 `omk install`을 실행하면 AI 에이전트 환경(`~/.gemini/skills/omk/SKILL.md` 및 `~/.gemini/config/mcp_config.json`)에 자동으로 등록됩니다. 이후에는 사용자가 CLI를 직접 호출할 필요 없이 AI 에이전트와 자연어로 대화하기만 하면 에이전트가 스스로 지식을 검색하고 축적합니다.

## 문서 및 지식 맵 (Documentation)

전체 프로젝트 거버넌스 및 상세 설계는 [`AGENTS.md`](AGENTS.md)의 지식 맵을 참고하십시오:
- [`CONSTITUTION.md`](CONSTITUTION.md): 프로젝트 헌법 및 설계 원칙
- [`docs/architecture.md`](docs/architecture.md): 시스템 세부 아키텍처 (CQRS, Daemon-Proxy, Storage Isolation)
- [`docs/protocol_spec.json`](docs/protocol_spec.json): MCP 및 JSON-RPC 2.0 통신 프로토콜 명세
- [`docs/requirements/index.md`](docs/requirements/index.md): 제품 요구사항 및 Epic 스펙 진입점

## 🙏 감사의 말 (Acknowledgments)

이 프로젝트는 [Andrej Karpathy의 LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) 아이디어와 개념에서 깊은 영감을 받았습니다.

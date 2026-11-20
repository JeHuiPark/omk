# omk (oh my knowledge) - Architecture & System Design

[`CONSTITUTION.md`](../CONSTITUTION.md)의 설계 철학을 구현하기 위한 시스템 아키텍처 명세.
(시각화된 전체 구조 다이어그램은 [`docs/architecture.excalidraw`](architecture.excalidraw) 참조)

## 1. 기술 스택 (Technology Stack)
- **Core Language:** Rust (메모리 안전성, GC 없는 고성능, 정적 단일 바이너리 빌드 지원)
- **Communication Data Format:** JSON (MCP 표준 JSON-RPC 2.0 규격 사용)
- **Testing Framework:** 표준 빌트인 테스트 (`#[cfg(test)]`) 및 Trait 기반 의존성 주입(Dependency Injection) 모킹

## 2. 통신 아키텍처 (Communication Architecture: Daemon & Proxy)
동시성 제어(Race Condition 방지)와 에이전트 호환성을 모두 충족하기 위해 **글로벌 데몬 + 프록시 패스스루** 패턴을 사용합니다.
- **`omk daemon` (글로벌 싱글톤 데몬):**
  - 로컬 백그라운드에서 상주하며 단일 프로세스로서 모든 파일/DB 접근을 통제합니다.
  - 구동 시 PID 파일 및 OS 레벨 파일 락(`flock`)을 획득하여 다중 프록시가 동시에 기동을 시도하더라도 단일 인스턴스(Singleton)만 실행됨을 보장합니다.
  - 내부적으로 Unix Domain Socket(또는 로컬 IPC)을 통해 프록시와 통신합니다.
- **`omk mcp` (Stdio 프록시):**
  - AI 클라이언트(에이전트)가 세션을 열 때마다 띄우는 서브프로세스입니다.
  - 파일이나 DB에 직접 접근하지 않으며, 에이전트의 Stdio 입출력을 `omk daemon`으로 포워딩(Forwarding)합니다.
  - 데몬이 실행 중이지 않을 경우 자동으로 데몬을 백그라운드 기동(Auto-spawn)한 뒤, 명시적 타임아웃(Deadline)과 지수적 백오프(Exponential Backoff) 재시도 전략을 통해 연결합니다.

## 3. 배포 및 실행 환경 (Deployment & Execution)
- **배포 형태:** 완전 독립형 단일 정적 바이너리 (Statically Linked Standalone Binary)
- **런타임 의존성:** 외부 런타임(Python, Node.js 등) 불필요
- **지원 타겟 (Target OS/Arch):**
  - macOS (Apple Silicon `aarch64` / Intel `x86_64`)
  - Windows (x64) / Linux (x64, `aarch64`)

## 4. 소프트웨어 아키텍처: AI Memory Pipeline (CQRS)
AI 지식 금고의 특성상 쓰기(저장/인덱싱)와 읽기(검색/조회)를 완벽히 분리하기 위해 **CQRS(Command Query Responsibility Segregation)** 파이프라인 아키텍처를 채택합니다. 이 파이프라인은 `omk daemon` 내부에서 실행됩니다.

### 4.1 Transport Layer, Concurrency & Backpressure
- **CQRS 큐 분리:**
  - **Command Queue (쓰기):** 단일 직렬화 큐(Single-writer Serialization)를 통해 처리하여 파일 시스템 및 인덱스 갱신 시 Race Condition과 데드락을 원천 차단합니다.
  - **Query Workers (읽기):** SQLite **WAL(Write-Ahead Logging) 모드**를 활성화하여 쓰기 트랜잭션 중에도 블로킹 없이 병렬로 조회를 처리합니다.
- **백프레셔(Backpressure):** 데몬 내부 요청 큐가 임계치를 초과하면 즉시 Fail-fast 에러(`-32001 Server Busy`)를 반환하여 프록시 및 에이전트의 무한 대기를 방지합니다.
- **관측성(Observability):** 백그라운드 데몬은 표준 입출력을 오염시키지 않도록 지정된 로그 파일(또는 `stderr`)에 JSON 구조화 로그를 기록합니다.

### 4.2 Command Pipeline (Ingestion / 쓰기 파이프라인)
지식을 저장소에 기록하고 인덱싱하는 단방향 흐름입니다.
- **Validator:** 메타데이터, 출처(Source Class/Type), 권위(Authority Score) 정책 유효성 검증 (`USER_EXPLICIT` 지식을 `AGENT_INFERRED` 요청이 임의로 덮어쓰지 못하도록 보호).
- **Vault Writer:** 동기화 전략(`UPSERT_FULL`, `APPEND_ONLY`, `STATE_PATCH`, `LINK_REFERENCE`)에 따라 마크다운 문서를 갱신(`DocumentStore` 사용).
- **Indexer:** 메타데이터와 본문을 추출하여 인덱스 DB에 반영(`IndexStore` 사용).

### 4.3 Query Pipeline (Retrieval / 읽기 파이프라인)
저장소(Vault 파일) 대신 인덱스 DB를 우선 조회하여 초고속으로 컨텍스트를 추출하거나 단건 문서를 조회하는 흐름입니다.
- **Query Parser:** 에이전트의 질의를 FTS5 검색 쿼리와 메타데이터 필터 조건(`tags`, `source_class` 등)으로 변환.
- **Retriever:** 조건에 맞는 문서를 조회(`IndexStore` 및 단건 조회 시 `DocumentStore` 사용).
- **Synthesizer:** 에이전트가 소비하기 쉬운 포맷(Snippet + Metadata)으로 결과를 정제하여 반환.

## 5. 스토리지 계층 격리 및 무결성 (Storage Abstraction & Integrity)
파이프라인이 물리적 저장소에 직접 결합되지 않도록 Rust Trait으로 완전히 격리합니다.

### 5.1 DocumentStore (단일 진실 공급원 - SSOT)
- **역할:** 원본 지식 문서(Markdown + YAML Frontmatter)의 영구 저장을 담당 (`LocalMarkdownDocumentStore`).
- **원자적 쓰기(Atomic Write):** 파일 작성 시 임시 파일(`.tmp`)에 먼저 기록하고 `fsync` 후 원자적 이름 변경(`rename`)을 수행하여 프로세스 크래시 시에도 파일 손상을 방지합니다.
- **경로 보안(Path Traversal 방지):** 모든 문서 ID 및 파일 경로는 정규화(Canonicalization/Sanitization)를 거쳐 Vault 루트 디렉터리 외부 접근을 엄격히 차단합니다.

### 5.2 IndexStore (검색 인덱스 DB)
- **역할:** 초고속 본문 검색(FTS5) 및 메타데이터(JSON) 필터링을 담당 (`SqliteFtsIndexStore`).

### 5.3 이종 스토리지 정합성 및 자가 복구 (Reconciliation / Auto-Recovery)
- **트랜잭션 조율:** `DocumentStore`(SSOT)에 원자적 쓰기를 선행한 후 `IndexStore`를 갱신합니다.
- **구동 시 자가 복구(`reconcile`):** 데몬 기동 시(또는 복구 명령 실행 시) Vault 내 마크다운 파일의 버전 해시(`version_hash`)와 인덱스 DB 상태를 대사(Reconciliation)하여, 누락되거나 불일치하는 인덱스를 자동으로 재구축합니다.

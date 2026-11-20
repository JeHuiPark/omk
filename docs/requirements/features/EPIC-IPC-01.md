# EPIC-IPC-01: MCP 통신 프로토콜 및 생명주기 (Vault Server & Session Proxy Lifecycle)

## 1. Introduction
### 1.1 Purpose
본 스펙은 호환성이 뛰어난 Stdio 기반 **MCP(Model Context Protocol, JSON-RPC 2.0)**를 제공하면서도, 내부적으로는 백그라운드 **Vault Server**를 통해 동시성 및 프로세스 생명주기 문제를 완벽히 해결하는 이중 통신 구조를 정의합니다.

### 1.2 Scope
- Vault Server(`omk daemon`)와 Session Proxy(`omk mcp`)의 역할, 프로세스 락, 기동 및 복구 생명주기.
- MCP Server 통신 규격(`initialize`, `notifications/initialized`, `ping`, `tools/list`, `tools/call`) 및 레거시/직접 JSON-RPC 메서드 호환.
- 타임아웃, 백프레셔, 표준화된 에러 코드 체계.

## 2. Overall Description
### 2.1 Product Perspective
다중 AI 클라이언트(Claude, Cursor 등)가 세션마다 띄우는 프로세스(`omk mcp`)는 **Session Proxy** 역할을 수행하며, 실제 파일 및 DB 접근 권한과 비즈니스 로직은 백그라운드 **Vault Server**(`omk daemon`)가 독점 수행합니다. 또한 CLI를 통한 스킬 프로비저닝(`omk install`) 및 직접 제어 명령을 지원합니다.

## 3. System Features

### 3.1 REQ-IPC-001: 시스템 생명주기 및 Vault Server 관리
- **3.1.1 Description and Priority:** 매우 높은 우선순위 (코어 인프라).
- **3.1.2 Functional Requirements:**
  - **Vault Server (`omk daemon`):**
    - PID 파일 및 OS 파일 락(`flock`)을 통해 지정된 Vault 경로당 단 하나의 서버 인스턴스만 실행되도록 보장해야 한다.
    - 기동 시 `DocumentStore`와 `IndexStore` 간의 정합성 대사(Reconciliation)를 수행하여 불일치 인덱스를 자가 복구해야 한다.
    - Unix Domain Socket(UDS)을 통해 Session Proxy들의 연결을 수신하고 처리해야 한다.
  - **Session Proxy (`omk mcp`):**
    - 에이전트에 의해 서브프로세스로 실행되며, 데몬 소켓 연결 실패 시 데몬을 백그라운드로 자동 기동(Auto-spawn)한 후 지수적 백오프(Exponential Backoff)로 재연결을 시도해야 한다.
    - 데몬으로의 요청 전송 및 응답 수신에 명시적 타임아웃(기본 30초)을 적용하여 무한 대기를 방지해야 한다.

### 3.2 REQ-IPC-002: MCP 프로토콜 및 Tools 노출 (CQRS 통합)
- **3.2.1 Description and Priority:** 높은 우선순위.
- **3.2.2 Functional Requirements:**
  - MCP 표준 핸드셰이크(`initialize`, `notifications/initialized`, `ping`)를 지원해야 한다.
  - `tools/list` 요청에 대해 다음 3가지 핵심 도구의 JSON Schema를 반환해야 한다:
    1. `upsert_knowledge`: 지식 생성 및 동기화 전략 기반 갱신 (Command Pipeline)
    2. `search_knowledge`: FTS5 전문 검색 및 메타데이터 필터링 (Query Pipeline)
    3. `read_knowledge`: ID 기반 지식 단건 원문 및 메타데이터 조회 (Query Pipeline)
  - `tools/call` 요청뿐만 아니라 직접 메서드 호출(`upsert_knowledge`, `search_knowledge`, `read_knowledge`)도 호환 처리하여 에이전트 및 테스트 클라이언트의 범용성을 보장해야 한다.
  - 데몬은 쓰기 요청(`upsert_knowledge`)을 직렬화하여 처리하고, 읽기 요청(`search_knowledge`, `read_knowledge`)은 WAL 모드를 통해 병렬로 처리해야 한다.

### 3.3 REQ-IPC-003: 엔터프라이즈 에러 핸들링 및 백프레셔 체계
- **3.3.1 Description and Priority:** 매우 높은 우선순위 (장애 추적 및 안정성).
- **3.3.2 Functional Requirements:**
  - 모든 에러는 프로세스 패닉 없이 표준 JSON-RPC 2.0 에러 포맷으로 반환되어야 한다:
    - `-32700`: Parse Error (잘못된 JSON 형식)
    - `-32600`: Invalid Request (유효하지 않은 JSON-RPC 요청 규격)
    - `-32601`: Method Not Found (존재하지 않는 메서드/도구)
    - `-32602`: Invalid Params (필수 파라미터 누락 또는 타입 불일치)
    - `-32603`: Internal Error (내부 스토리지 또는 시스템 에러)
    - `-32000`: Daemon Unreachable / Timeout (데몬 연결 불가 또는 응답 시간 초과)
    - `-32001`: Server Busy / Backpressure (데몬 요청 큐 포화로 인한 거절)
    - `-32002`: Authority Violation (예: `AGENT_INFERRED`가 `USER_EXPLICIT` 지식을 덮어쓰려 시도함)
    - `-32003`: Path Traversal / Access Denied (허용되지 않은 경로 접근 시도)
    - `-32004`: Knowledge Not Found (요청한 ID의 문서가 존재하지 않음)

# EPIC-PERF-01: 가상 워크로드 기반 성능 고도화 및 정적/결정론적 검증 지표 명세 (Workload-Driven Performance Optimization & Static/Deterministic Verification Metrics)

## 1. Introduction

### 1.1 Purpose
본 스펙은 [`EPIC-OBS-01`](EPIC-OBS-01.md)을 통해 구축된 실행 관측성 인프라와 6대 병목 진단 결과를 바탕으로, 실제 파워 유저의 장기 사용 조건(**주 5일, 일 8시간, 시간당 평균 10 사용자 턴**)에서 `omk` 시스템이 일관된 저지연(Low-Latency) 응답성과 높은 검색 회수율(Recall)을 유지하도록 **코어 및 Vault Server / Session Proxy 아키텍처의 성능 개선 요구사항**을 정의합니다.

특히 CI/CD 환경이나 개발자 로컬 머신의 일시적인 CPU/디스크 부하에 따라 흔들리는 단순 벽시계 시간(Wall-clock Time) 측정의 한계를 극복하기 위해, **코드 및 아키텍처 수준에서 결정론적으로 카운팅하고 단언(Assert)할 수 있는 정적/구조적 복잡도 지표(Static & Invariant Complexity Metrics)**와 **1년 치 누적 금고 규모를 재현하는 합성 워크로드 벤치마크 SLA(Synthetic Workload Benchmark SLAs)**를 함께 명세합니다.

### 1.2 Scope
- **가상 조건 기반 정량 워크로드 모델링 (Quantitative Workload Modeling):** 주 5일 × 일 8시간 × 시간당 10턴 조건에서의 일/주/월/연간 RPC 트래픽, 도구별 호출 분포, 로컬 금고(Vault) 문서 및 SQLite 인덱스 증가 수식 도출.
- **정적/결정론적 성능 지표 (Static & Deterministic Metrics) 정의:** 하드웨어 속도와 무관하게 $O(1)$ 또는 상한 상수($C$)로 검증 가능한 시스템 콜, 소켓 연결, DB 핸들 오픈, 트랜잭션 횟수, 파일 스캔 복잡도 불변식(Invariants) 정의.
- **6대 지연 병목 구간의 구조적 해소 요구사항 (`REQ-PERF-001` ~ `REQ-PERF-005`):**
  1. Session Proxy(`omk mcp`) 세션 내 영속 UDS 연결 재사용 및 Vault Server(`omk daemon`) 이벤트 기반 블로킹 `accept` 루프 전환(15ms 폴링 슬립 지터 제거).
  2. Vault Server(`omk daemon`) 콜드 스타트 시 소켓 선(先) 바인딩(Fast-Bind) 및 증분 메타데이터(`mtime`/`size`) 기반 백그라운드 `reconcile` + 배치 트랜잭션 인덱싱.
  3. SQLite Reader 커넥션 재사용(Pool/Thread-Local) 및 SQL 푸시다운 필터링(`WHERE` 절 조건 위임)을 통한 후처리 JSON 파싱 제거.
  4. 다중 키워드 검색 시 Zero-Hit 방지를 위한 2단계 자동 `OR` 폴백(BM25 랭킹 기반).
  5. 쓰기 파이프라인(`upsert_knowledge`) 내 불필요한 중복 파일 읽기/직렬화 최소화 및 내구성(Durability) 정책 제어.
- **결정론적 성능 검증 및 합성 벤치마크 하네스 (`REQ-PERF-006`):** `omk bench` CLI 및 카운터 기반 정적 불변식 단위/통합 테스트 규격 정의.

### 1.3 Definitions, Acronyms, and Abbreviations
- **User Turn (사용자 턴):** 인간 사용자가 AI 에이전트에게 프롬프트를 입력하고 최종 답변을 받기까지의 1회 대화 주기. 단일 사용자 턴 내에서 에이전트는 선제적 교차 노출(Proactive Grounding)을 위해 0~N회의 `omk` MCP 도구 호출을 수행한다.
- **Static / Deterministic Metric (정적/결정론적 지표):** OS 스케줄러나 디스크 상태에 좌우되는 실행 시간(`ms`)이 아니라, 주어진 입력 크기($N$개 문서, $M$개 요청)에 대해 실행되는 **함수/시스템 콜 호출 횟수, I/O 바이트 수, 트랜잭션 수, 슬립 횟수** 등 실행 환경과 무관하게 항상 동일한 값을 산출하여 테스트 코드에서 `assert_eq!` 또는 상한 부등식으로 검증 가능한 지표.
- **Retry Churn (재질의 루프):** `search_knowledge` 결과가 0건(`zero_hit: true`)이거나 부적절할 때 AI 에이전트가 검색어를 바꾸어 연속으로 재호출하는 현상으로, 매 재시도마다 LLM 추론 왕복 지연(2~5초)을 유발하는 최대 체감 병목 요인.
- **SQL Filter Pushdown (SQL 필터 푸시다운):** `source_class`, `source_type`, `min_authority`, `tags` 필터를 SQLite 쿼리 결과 인출 후 애플리케이션(Rust)에서 JSON 파싱으로 걸러내는 대신, SQL `WHERE` 절과 인덱스를 통해 스토리지 엔진 내부에서 선행 필터링하는 기법.
- 기타 핵심 도메인 용어(`omk CLI`, `omk Skill`, `Local Vault` 등)는 [용어 사전 (Glossary)](../glossary.md)을 따릅니다.

### 1.4 References
- [`CONSTITUTION.md`](../../../CONSTITUTION.md) : Environment Isolation, Core Isolation, Zero Heavy Dependencies 원칙
- [`docs/architecture.md`](../../architecture.md) : Vault Server & Session Proxy 통신 구조 및 CQRS AI Memory Pipeline 아키텍처
- [`docs/requirements/glossary.md`](../glossary.md) : 프로젝트 용어 사전 및 도메인 경계
- [`docs/requirements/features/EPIC-OBS-01.md`](EPIC-OBS-01.md) : 프라이버시 보호 기반 실행 관측성 및 6대 지연 후보 구간 진단 (`DEF-OBS-01` ~ `DEF-OBS-06`)
- [`docs/requirements/features/EPIC-IPC-01.md`](EPIC-IPC-01.md) : MCP 통신 프로토콜 및 Vault Server & Session Proxy 생명주기
- [`docs/requirements/features/EPIC-SEARCH-01.md`](EPIC-SEARCH-01.md) : 지식 검색 및 SQLite FTS5 인덱싱 명세
- [`docs/requirements/features/EPIC-VAULT-01.md`](EPIC-VAULT-01.md) : Markdown 기반 영구 지식 저장소 명세

---

## 2. Overall Description

### 2.1 Quantitative Persona Workload Modeling (가상 조건 기반 정량 워크로드 모델링)

#### 2.1.1 사용자 활동 및 턴(Turn) 볼륨 수식
타겟 페르소나는 **주 5일, 하루 8시간** 동안 AI 에이전트를 활용하여 개인 업무(설계, 코딩, 문서화, 의사결정 기록)를 수행하며, **시간당 평균 10회의 사용자 턴($T_{\text{hour}} = 10$)**을 소비합니다.

$$\begin{aligned}
T_{\text{hour}} &= 10 \text{ turns/hour} \\
T_{\text{day}} &= 8 \text{ hours/day} \times 10 \text{ turns/hour} = 80 \text{ turns/day} \\
T_{\text{week}} &= 5 \text{ days/week} \times 80 \text{ turns/day} = 400 \text{ turns/week} \\
T_{\text{month}} &\approx 21.67 \text{ days/month} \times 80 \text{ turns/day} \approx 1,733 \text{ turns/month} \quad (\text{기준치: } 1,700 \text{ turns}) \\
T_{\text{year}} &= 250 \text{ days/year} \times 80 \text{ turns/day} = 20,000 \text{ turns/year}
\end{aligned}$$

#### 2.1.2 턴당 `omk` 도구 호출 빈도 및 일간/연간 RPC 트래픽 모델
[`SKILL.md`](EPIC-CLI-01.md)의 **Proactive Grounding(선제적 교차 노출)** 지침에 따라, AI 에이전트는 사용자 턴마다 맥락 검색(`search_knowledge`), 상세 원문 조회(`read_knowledge`), 의사결정/규칙 축적(`upsert_knowledge`)을 수행합니다.

| 도구 / 이벤트 | 턴당 호출 확률 / 빈도 ($\lambda_i$) | 일간 호출 수 (80턴 기준) | 주간 호출 수 (400턴) | 월간 호출 수 (~1,700턴) | 연간 호출 수 (20,000턴) | 비고 (호출 발생 맥락) |
| :--- | :---: | :---: | :---: | :---: | :---: | :--- |
| **`search_knowledge`** | $0.75$ 회/턴 | **60 회/일** | 300 회/주 | 1,275 회/월 | **15,000 회/년** | 질문 응답 전 선제적 배경 지식 검색 (Zero-Hit 재시도 발생 시 최대 $1.25$회/턴 $\rightarrow$ 100회/일로 급증) |
| **`read_knowledge`** | $0.50$ 회/턴 | **40 회/일** | 200 회/주 | 850 회/월 | **10,000 회/년** | 검색 적중 시 상위 1~2개 핵심 문서의 전체 본문 조회 |
| **`upsert_knowledge`** | $0.25$ 회/턴 | **20 회/일** | 100 회/주 | 425 회/월 | **5,000 회/년** | 사용자의 명시적 규칙(`USER_EXPLICIT`) 또는 작업 결과 요약(`AGENT_INFERRED`) 저장 |
| **MCP 제어/핸드셰이크** | 세션당 $3$ 회 | **15 회/일** | 75 회/주 | 320 회/월 | **3,750 회/년** | 하루 평균 5회 IDE/에이전트 세션 시작 (`initialize`, `notifications/initialized`, `tools/list`) |
| **합계 ($M_{\text{total}}$)** | **$1.50$ 도구/턴** | **135 RPC/일**<br>*(도구 120회)* | **675 RPC/주**<br>*(도구 600회)* | **2,870 RPC/월**<br>*(도구 2,550회)* | **33,750 RPC/년**<br>*(도구 30,000회)* | 평균 요청 간격: 근무 시간 중 약 **3.6분(216초)**마다 도구 호출 집중 발생 (버스트 시 연속 호출) |

#### 2.1.3 시간에 따른 로컬 금고(Vault) 규모 및 스토리지 성장 모델
하루 평균 $U_{\text{day}} = 20$회의 `upsert_knowledge` 호출 중, **신규 문서 생성 비율을 $\alpha_{\text{new}} = 0.60$ (일 12건 생성)**, **기존 문서 갱신/병합(`UPSERT_FULL`, `APPEND_ONLY`, `STATE_PATCH`) 비율을 $1 - \alpha_{\text{new}} = 0.40$ (일 8건 갱신)**으로 모델링합니다. 문서당 평균 초기 크기를 $S_0 = 2.5\text{ KB}$, `APPEND_ONLY` 및 `STATE_PATCH` 누적으로 인한 평균 문서 크기를 $\bar{S}(t)$라 할 때, 경과 근무일 $d$에 따른 누적 문서 수 $N(d) = \alpha_{\text{new}} \cdot U_{\text{day}} \cdot d = 12 \cdot d$는 다음과 같습니다.

| 누적 시점 (근무일 $d$) | 누적 사용자 턴 ($T$) | 누적 쓰기 횟수 ($U$) | 누적 고유 문서 수 $N(d)$ | 평균 문서 크기 ($\bar{S}$) | Vault 디스크 용량 ($\approx N \cdot \bar{S}$) | SQLite 인덱스 크기 (`index.db` + WAL) | 현행 아키텍처의 `reconcile` 전체 스캔 비용 (예측) |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :--- |
| **1주 차 ($d = 5$)** | 400 턴 | 100 회 | **60 개** | $2.6\text{ KB}$ | $\sim 0.16\text{ MB}$ | $\sim 0.3\text{ MB}$ | 60개 `.md` 전체 읽기 + 60개 JSON 파싱 ($\sim 8\text{ ms}$) |
| **1개월 차 ($d = 21$)** | 1,700 턴 | 425 회 | **250 개** | $3.0\text{ KB}$ | $\sim 0.75\text{ MB}$ | $\sim 1.2\text{ MB}$ | 250개 `.md` 전체 읽기 + 250개 JSON 파싱 ($\sim 35\text{ ms}$) |
| **6개월 차 ($d = 125$)** | 10,000 턴 | 2,500 회 | **1,500 개** | $4.0\text{ KB}$ | $\sim 6.0\text{ MB}$ | $\sim 9.0\text{ MB}$ | 1,500개 `.md` 전체 읽기 + 1,500개 JSON 파싱 ($\sim 220\text{ ms}$) |
| **1년 차 ($d = 250$)** | 20,000 턴 | 5,000 회 | **3,000 개** | $4.5\text{ KB}$ | $\sim 13.5\text{ MB}$ | $\sim 20.0\text{ MB}$ | 3,000개 `.md` 전체 읽기 + 3,000개 JSON 파싱 ($\sim 450\text{ ms}$) |
| **파워 유저 상한 ($1\text{년 Max}$)** | 20,000 턴 | 5,000 회 ($\alpha_{\text{new}}=1.0$) | **5,000 개** | $5.0\text{ KB}$ | $\sim 25.0\text{ MB}$ | $\sim 35.0\text{ MB}$ | 5,000개 `.md` 전체 읽기 + 5,000개 JSON 파싱 ($\sim 800\text{ ms+}$) |

#### 2.1.4 하루 8시간 근무 중 Vault Server 기동 및 유휴(Lifecycle) 패턴
- **콜드 스타트(Cold Start) 빈도:** 사용자는 출근 직후 첫 AI 세션 시작 시 1회, 그리고 오후 중 IDE 재시작/브랜치 전환/수동 종료 등으로 하루 평균 **2~5회**의 Vault Server(`omk daemon`) 콜드 스타트를 경험합니다.
- **Session Proxy 지속 시간:** 한 번 열린 AI 에이전트 하네스(Session Proxy, `omk mcp`)는 평균 **1~4시간** 동안 유지되며, 단일 프록시 프로세스 수명 동안 평균 **25~60회의 연속 RPC 요청**을 포워딩합니다.

---

### 2.2 Structural Bottleneck Verification against Codebase (현행 코드베이스의 구조적 병목 검증)

[`EPIC-OBS-01`](EPIC-OBS-01.md) §2.1에서 진단된 6대 지연 후보 구간을 실제 구현 코드(`skill/src/proxy/mod.rs`, `skill/src/daemon/mod.rs`, `skill/src/protocol/mod.rs`, `core/src/search/mod.rs`, `core/src/vault/mod.rs`, `core/src/index/mod.rs`)와 대조한 결과, 다음과 같은 **비효율적 시간/공간 복잡도와 고정 지연(Structural Overhead)**이 실재함이 확인되었습니다.

| 병목 번호 | 관련 결함 ID | 코드 위치 및 현행 구현 메커니즘 | 정적 복잡도 (현행) | 가상 워크로드(1년 차, $N=3,000$, 일 $M=120$ 요청)에서의 누적 손실 |
| :--- | :--- | :--- | :---: | :--- |
| **병목 1**<br>(IPC 연결) | `DEF-OBS-01` | `skill/src/proxy/mod.rs:193` (`forward_line_timed`)<br>단일 `McpProxy` 프로세스 내에서 매 줄(`read_line`)마다 `connect_with_backoff_timed()`를 호출하여 매번 신규 `UnixStream::connect()` 생성 후 즉시 닫음(`EOF`). 데몬(`daemon/mod.rs:370`)도 매 요청마다 `thread::spawn`을 반복함. | 요청 $M$개당<br>`connect()` **$O(M)$회**<br>`thread::spawn` **$O(M)$회** | 세션 내 120회 요청마다 소켓 생성/해제 및 스레드 생성 오버헤드 반복 + **병목 3의 `accept` 슬립 지터를 매 요청마다 강제 유발** |
| **병목 2**<br>(콜드 스타트) | `DEF-OBS-02` | `skill/src/daemon/mod.rs:306, 328-334` (`DaemonServer::initialize`, `run_blocking`)<br>`PidLockGuard::acquire()` 및 `UnixListener::bind()` **이전에** `pipeline.reconcile_timed()`를 동기 실행함. 이때 `list_documents()`(`vault/mod.rs:474`)가 모든 `.md` 파일 본문을 읽고 파싱하며, 재인덱싱 시 `upsert_index()`를 문서별 개별 트랜잭션으로 호출함. | 소켓 바인딩 전<br>파일 읽기 **$O(N)$회**<br>재인덱싱 트랜잭션 **$O(K)$회** | $N=3,000$ 문서 시 소켓 생성이 수백 ms 지연되어 프록시가 백오프 슬립(`25ms → 50ms → 100ms → 200ms...`)에 진입, 첫 요청 지연이 **$375\text{ms} \sim 1,575\text{ms}$**로 증폭됨 |
| **병목 3**<br>(수신 폴링) | `DEF-OBS-03` | `skill/src/daemon/mod.rs:341, 381`<br>`listener.set_nonblocking(true)` 설정 후 `accept()`가 `WouldBlock`일 때 무조건 `thread::sleep(Duration::from_millis(15))` 수행. | 신규 연결당<br>**$0 \sim 15\text{ ms}$ 고정 슬립**<br>(기댓값 $7.5\text{ ms}$) | 병목 1(매 요청 신규 연결)과 결합하여 **모든 단일 RPC 요청마다 평균 $7.5\text{ ms}$, P95 $14.25\text{ ms}$의 순수 인공 지연(Artificial Jitter)** 발생 (일 120회 호출 시 매일 $900\text{ ms}$ 허비, 유휴 시 초당 66회 불필요한 깨어남 발생) |
| **병목 4**<br>(DB 재오픈 및 후처리 필터) | `DEF-OBS-05` | `core/src/index/mod.rs:375, 538, 620`<br>1) 매 `search_timed()` 및 `list_index_hashes()` 호출 시 `open_reader()`가 `sqlite3_open()`과 `PRAGMA` 2건을 매번 새로 실행함.<br>2) `WHERE` 절에 `source_class`, `source_type`, `authority` 조건을 넣지 않고 `LIMIT ?2`(`limit * 4`)만 가져온 뒤 Rust 루프(`collect_filtered_rows`)에서 `JsonValue::parse(&meta_raw)`로 필터링함. | 검색 $Q$회당<br>`sqlite3_open` **$O(Q)$회**<br>JSON 파싱 **$O(Q \cdot L)$회** | 1) 매 검색마다 커넥션 오픈/스키마 파싱 비용 발생.<br>2) 상위 `limit * 4`(예: 20개) 행에 해당 태그/클래스가 없으면 **실제 DB에 매칭 문서가 수백 개 있어도 0건을 반환하는 심각한 검색 누락(False Zero-Hit)** 발생! |
| **병목 5**<br>(Zero-Hit 재질의 루프) | `DEF-OBS-06` | `core/src/index/mod.rs:382` (`build_fts_query`)<br>공백 구분 다중 키워드(예: `"결제 아키텍처 타임아웃 정책"`)를 무조건 `"결제" AND "아키텍처" AND "타임아웃" AND "정책"`으로 변환함. 단어 하나만 본문에 없어도 `0`건 반환. | 다중 토큰 질의 시<br>단일 `AND` 패스만 수행<br>(`OR` 폴백 부재) | 에이전트가 3~5개 키워드로 질문 시 높은 확률로 `zero_hit: true` 발생 $\rightarrow$ 에이전트가 검색어를 줄여가며 2~3회 연속 재호출 $\rightarrow$ **LLM 턴 지연 $3,000 \sim 8,000\text{ ms}$ 추가 발생** |
| **병목 6**<br>(쓰기 I/O 및 락 점유) | `DEF-OBS-04`<br>`DEF-OBS-05` | `core/src/search/mod.rs:117, 240`, `core/src/index/mod.rs:359`<br>1) `upsert_knowledge`가 `command_lock` 획득 상태에서 디스크 읽기 + `.tmp` 쓰기 + `fsync` + SQLite 트랜잭션을 직렬 수행.<br>2) `knowledge_index` 테이블에 `version_hash`와 `tags` 전용 컬럼이 없어 `list_index_hashes()`가 매번 전체 `metadata` JSON을 파싱함. | `reconcile` 시<br>JSON 파싱 **$O(N)$회** | 대사(`reconcile`) 및 연속 쓰기 시 `command_lock` 점유 시간이 길어져 후속 쓰기 요청의 `lock_wait_us` 증가 |

---

### 2.3 Static & Deterministic Performance Metrics Framework (정적/결정론적 성능 측정 및 검증 프레임워크)

성능 개선의 달성 여부를 객관적으로 판정하기 위해, 본 스펙은 **계층 A: 정적/결정론적 불변식 지표(Static Invariant Metrics — 환경 무관 100% 재현)**와 **계층 B: 합성 워크로드 동적 SLA 지표(Synthetic Workload SLA Metrics — `omk bench` 검증)**의 이중 검증 체계를 도입합니다.

#### 2.3.1 계층 A: 정적/결정론적 구조 복잡도 지표 (Static & Invariant Complexity Metrics)
코드 내 원자적 카운터(Atomic Instrumentation Counters)를 통해 단위/통합 테스트에서 실행 머신의 CPU 속도와 무관하게 **정확한 횟수(`==` 또는 `<=`)로 단언(Assert)**할 수 있는 지표입니다.

| 지표 ID | 지표 명칭 (Metric Name) | 측정 대상 동작 | 현행 수치 (AS-IS) | 목표 정적 불변식 (TO-BE Target Invariant) | 검증 방법 |
| :--- | :--- | :--- | :---: | :---: | :--- |
| **`MET-STATIC-01`** | **세션 내 소켓 연결 복잡도**<br>(`uds_connect_count_per_session`) | 단일 `McpProxy` 인스턴스에서 $M$개의 연속 JSON-RPC 요청 포워딩 ($M \ge 1$, 데몬 정상 구동 중) | $M$ 회<br>($O(M)$) | **$1$ 회 ($O(1)$)**<br>*(재연결 0회, 단일 영속 `UnixStream` 재사용)* | 통합 테스트에서 $M=50$ 연속 요청 후 데몬의 누적 `accepted_connections == 1` 단언 |
| **`MET-STATIC-02`** | **데몬 수신 루프 인공 슬립 지연**<br>(`accept_poll_sleep_ms`) | 데몬 유휴 상태에서 신규 클라이언트 소켓 연결 수락 시 발생하는 폴링 슬립 횟수 및 시간 | 연결당 $0 \sim 1$회<br>($15\text{ ms}$ 고정 슬립) | **$0$ 회 ($0\text{ ms}$)**<br>*(블로킹 `accept` 또는 OS 이벤트 멀티플렉싱으로 폴링 슬립 완전 제거)* | 코드 정적 검사(`listener` 루프 내 `sleep(15ms)` 부재) 및 연결 수락 지연 측정 |
| **`MET-STATIC-03`** | **콜드 스타트 바인딩 전 동기 파일 스캔 수**<br>(`pre_bind_full_file_reads`) | $N$개 문서가 존재하는 Vault에서 데몬 기동 시 UDS 소켓 `bind()` 완료 전까지 본문을 읽는 `.md` 파일 수 | $N$ 개<br>($O(N)$) | **$0$ 개 ($O(1)$)**<br>*(소켓 `bind()` 및 `flock` 획득을 `reconcile` 이전에 즉시 수행)* | $N=3,000$ Vault에서 데몬 기동 시 첫 `ping` 응답까지 `connect_retries <= 1` 단언 |
| **`MET-STATIC-04`** | **무변경 Vault 증분 대사(`reconcile`) 시 본문 파싱 횟수**<br>(`unchanged_reconcile_body_parses`) | $N$개 문서 중 변경된 파일이 $C$개($C \ll N$, 예: $C=0$)일 때 `reconcile`이 전체 `.md` 파일을 열어 파싱하는 횟수 | $N$ 회<br>($O(N)$) | **$C$ 회 ($O(C)$)**<br>*($C=0$일 때 파일 오픈/파싱 **$0$회**; 파일 메타데이터 `mtime`/`file_size`와 인덱스 대조로 스킵)* | $N=1,000$ 무변경 상태에서 `reconcile()` 실행 시 `markdown_parse_count == 0` 단언 |
| **`MET-STATIC-05`** | **대사(`reconcile`) 재인덱싱 트랜잭션 횟수**<br>(`reconcile_sqlite_tx_count`) | `reconcile` 중 $K$개($K \ge 1$)의 문서를 재인덱싱하거나 고아 항목을 삭제할 때 발생하는 SQLite `BEGIN ... COMMIT` 횟수 | $K$ 회<br>($O(K)$) | **$1$ 회 ($O(1)$)**<br>*(단일 배치 트랜잭션 내에서 $K$개 문서 일괄 반영)* | $K=500$개 신규 문서 대사 시 SQLite 트랜잭션 커밋 횟수 `== 1` 단언 |
| **`MET-STATIC-06`** | **검색 시 SQLite 핸들 재오픈 횟수**<br>(`search_sqlite_open_count`) | $Q$회의 연속 `search_knowledge` 실행 시 호출되는 `sqlite3_open()` 시스템 콜 횟수 | $Q$ 회<br>($O(Q)$) | **워커/스레드당 최대 $1$회 ($O(1)$)**<br>*(캐시된 Reader 커넥션 풀 또는 Thread-local 커넥션 재사용; 2번째 검색부터 `sqlite3_open == 0`)* | $Q=100$회 연속 검색 시 총 `sqlite3_open` 호출 횟수 `<= PoolSize` (단일 스레드 시 `== 1`) 단언 |
| **`MET-STATIC-07`** | **검색 후처리 JSON 파싱 복잡도 및 필터 정확도**<br>(`search_post_filter_json_parses`) | $N$개 문서 중 필터 조건(`source_class`, `source_type`, `min_authority`, `tags`)과 일치하는 문서가 전체의 하위 1%에 위치할 때의 검색 회수율(Recall) 및 불필요한 탈락 행 JSON 파싱 수 | 상위 `limit * 4`행만 검사 후 탈락 시 **Recall $0\%$** (False Zero-Hit) | **SQL `WHERE` 절 선행 필터링으로 Recall $100\%$ 보장** 및 비매칭 행에 대한 Rust JSON 파싱 **$0$회** | $N=1,000$ 문서 중 권위가 낮은 5개만 특정 태그를 가질 때 태그 필터 검색 시 5건 모두 정확히 반환됨을 단언 |
| **`MET-STATIC-08`** | **다중 키워드 부분 일치 회수율 (Fallback Recall)**<br>(`multi_token_fallback_hit_rate`) | 문서에 $W$개 키워드 중 $W-1$개만 포함되어 있을 때($W \ge 2$, 명시적 불리언 연산자 없음), `search_knowledge`의 반환 결과 수 | $0$ 건<br>(`zero_hit: true`, strict `AND` 실패) | **$\ge 1$ 건 (`zero_hit: false`)**<br>*(1차 `AND` 결과 0건 시 자동으로 2차 `OR` + BM25 랭킹 폴백 쿼리 실행)* | 3개 키워드 중 2개만 포함된 문서에 대해 기본 검색 시 매칭 결과 반환 및 `fallback_used == true` 단언 |

#### 2.3.2 계층 B: 가상 워크로드 재현 벤치마크 SLA 지표 (Synthetic Benchmark SLAs)
1년 차 누적 워크로드(**문서 $N = 3,000$개, 평균 크기 $4.5\text{ KB}$, 하루 80 사용자 턴 = 120 도구 호출 시뮬레이션**)를 로컬 환경에서 실행했을 때 충족해야 하는 동적 지연 시간 상한(SLA)입니다.

| 대상 작업 / 시나리오 | 워크로드 조건 ($N = 3,000$ 문서 기준) | P50 목표 (Median) | P95 목표 | P99 목표 | 검증 도구 |
| :--- | :--- | :---: | :---: | :---: | :--- |
| **`search_knowledge` (E2E Warm)** | 키워드 1~4개 + 필터 조합 (일 60회 분포) | **$\le 1.5\text{ ms}$** | **$\le 4.0\text{ ms}$** | **$\le 8.0\text{ ms}$** | `omk bench` / `omk stats` |
| **`read_knowledge` (E2E Warm)** | 단건 마크다운 조회 (일 40회 분포, 평균 $4.5\text{ KB}$) | **$\le 0.8\text{ ms}$** | **$\le 2.5\text{ ms}$** | **$\le 5.0\text{ ms}$** | `omk bench` / `omk stats` |
| **`upsert_knowledge` (E2E Warm)** | 신규 생성 60% + 기존 갱신 40% (일 20회 분포) | **$\le 4.0\text{ ms}$** | **$\le 12.0\text{ ms}$** | **$\le 25.0\text{ ms}$** | `omk bench` / `omk stats` |
| **Vault Server Cold Start Ready Time** | Vault Server(`omk daemon`) 미구동 상태에서 첫 요청 도착 ~ 소켓 연결 완료 | **$\le 25.0\text{ ms}$** | **$\le 50.0\text{ ms}$** | **$\le 80.0\text{ ms}$** | `omk bench` |
| **Incremental `reconcile` (No-op)** | $N = 3,000$개 문서가 모두 최신 상태일 때 정합성 대사 | **$\le 15.0\text{ ms}$** | **$\le 30.0\text{ ms}$** | **$\le 50.0\text{ ms}$** | `omk bench` |
| **Full Rebuild `reconcile` (Cold Index)** | `index.db` 삭제 후 $N = 3,000$개 문서 전체 재인덱싱 | **$\le 400\text{ ms}$** | **$\le 800\text{ ms}$** | **$\le 1,200\text{ ms}$** | `omk bench` |
| **일일 80턴(120 RPC) 총 누적 시스템 시간** | 하루치 120회 RPC 호출의 순수 `omk` 소요 시간 총합 | **$\le 300\text{ ms}$ / day** | **$\le 600\text{ ms}$ / day** | **$\le 1,000\text{ ms}$ / day** | `omk bench` (8시간 업무 중 1초 미만) |

---

## 3. System Features

### 3.1 REQ-PERF-001: 영속 IPC 세션 연결 및 무지터(Zero-Jitter) 이벤트 수신 루프
- **3.1.1 Description and Priority:** 매우 높은 우선순위 (`DEF-OBS-01`, `DEF-OBS-03` 해결). 프록시-데몬 간 매 요청 소켓 재연결 오버헤드와 데몬 수신 루프의 15ms 폴링 슬립 지터를 원천 제거합니다.
- **3.1.2 Stimulus/Response Sequences:**
  - *Stimulus 1:* AI 에이전트 하네스가 `omk mcp` 프록시를 기동하고 세션 동안 $M$개의 JSON-RPC 요청을 순차 전송함.
  - *Response 1:* `McpProxy`는 첫 번째 요청에서 맺은 `UnixStream` 연결을 내부 상태에 유지(Keep-Alive)하여 후속 $M-1$개 요청에서 `connect()` 없이 즉시 송수신함. 만약 데몬 재시작 등으로 소켓 쓰기/읽기가 끊어진 경우에만 투명하게 1회 재연결(Auto-reconnect)을 수행함.
  - *Stimulus 2:* 프록시가 데몬 소켓에 연결을 시도하거나 데이터를 전송함.
  - *Response 2:* 데몬 리스너는 15ms 고정 슬립 폴링 없이 즉시($0\text{ ms}$ 스케줄링 지터) 연결을 수락하고, 영속 연결 스트림 위에서 멀티 라인 JSON-RPC 요청을 연속 처리함.
- **3.1.3 Functional Requirements:**
  - **프록시 영속 연결 재사용 (`skill/src/proxy/mod.rs`):**
    - `McpProxy`는 세션 수명 동안 열려 있는 `UnixStream` 핸들(및 `BufReader`)을 캐싱하여 재사용해야 한다 (`MET-STATIC-01`: 정상 세션 내 $M$개 요청 시 `UnixStream::connect` 호출 횟수 $= 1$).
    - 캐시된 스트림에 요청 프레임을 쓰거나 응답을 읽는 도중 `BrokenPipe`, `ConnectionReset`, 또는 `EOF`가 감지되면, 즉시 기존 스트림을 폐기하고 `connect_with_backoff_timed()`를 통해 재연결을 1회 시도하여 요청을 복구해야 한다.
  - **데몬 무지터(Zero-Jitter) `accept` 및 우아한 종료 (`skill/src/daemon/mod.rs`):**
    - `DaemonServer::run_blocking` 루프에서 `WouldBlock` 시 `thread::sleep(Duration::from_millis(15))`를 수행하는 폴링 방식을 제거해야 한다 (`MET-STATIC-02`: `accept_poll_sleep_ms == 0`).
    - 리스너는 블로킹 `accept()` 모드로 동작하여 새 연결 요청 도착 시 커널이 즉시 깨우도록 하되, `daemon/shutdown` 명령 수신 시에는 워커 스레드가 `shutdown` 플래그를 설정한 뒤 로컬 소켓에 더미 연결(`UnixStream::connect(&socket_path)`)을 1회 발생시켜(Self-wake / Wakeup Kick) 블로킹된 `accept()` 루프가 지연 없이 즉시 종료되도록 해야 한다.
  - **초기 백오프 주기 단축:**
    - 콜드 스타트 발생 시 프록시의 첫 재시도 대기 시간을 현행 `25ms`에서 **`5ms`**(`5ms → 10ms → 25ms → 50ms → 100ms → 200ms → 400ms → 800ms`)로 세분화하여, 데몬 소켓 바인딩 직후 프록시가 불필요하게 기다리는 잔여 슬립 시간을 최소화해야 한다.

### 3.2 REQ-PERF-002: 콜드 스타트 Fast-Bind 및 메타데이터 기반 증분/배치 `reconcile`
- **3.2.1 Description and Priority:** 매우 높은 우선순위 (`DEF-OBS-02` 해결). 문서 수가 수천 개로 늘어난 1년 차 금고에서도 데몬 콜드 스타트 지연이 $O(1)$로 일정하게 유지되도록 기동 순서와 대사(`reconcile`) 알고리즘을 혁신합니다.
- **3.2.2 Stimulus/Response Sequences:**
  - *Stimulus 1:* 3,000개의 마크다운 문서가 저장된 Vault에서 데몬이 꺼져 있을 때 에이전트가 첫 요청을 보냄.
  - *Response 1:* 자동 기동된 `omk daemon`은 전체 파일 스캔을 기다리지 않고 즉시 PID `flock` 획득 및 Unix Domain Socket `bind()`를 먼저 완료하여 프록시의 연결을 수락함 (`MET-STATIC-03`: 바인딩 전 파일 스캔 0건). 초기 대사(`reconcile`)는 백그라운드 스레드에서 비동기로 수행하거나 증분 메타데이터 비교를 통해 수 밀리초 내에 완료함.
  - *Stimulus 2:* 데몬 기동 또는 `omk reconcile` 실행 시 Vault 디렉터리에 3,000개의 `.md` 파일이 존재하며, 마지막 인덱싱 이후 수정된 파일이 0개임.
  - *Response 2:* 파이프라인은 각 파일의 OS 메타데이터(`mtime`, `file_size`)를 SQLite 인덱스에 기록된 값과 대조하여, 변경되지 않은 3,000개 파일의 마크다운 본문 읽기 및 YAML 파싱을 완전히 생략함 (`MET-STATIC-04`: `markdown_parse_count == 0`).
- **3.2.3 Functional Requirements:**
  - **Fast-Bind 기동 시퀀스 (`skill/src/daemon/mod.rs`):**
    - 데몬 프로세스 시작 시 `PidLockGuard::acquire()` 및 `UnixListener::bind()`를 `pipeline.reconcile()` **이전에** 수행하여, 프록시가 콜드 스타트 시 첫 백오프 주기(`5~15ms`) 내에 즉시 소켓에 연결될 수 있도록 보장해야 한다 (`MET-STATIC-03`).
    - 초기 `reconcile`은 소켓 바인딩 직후 백그라운드에서 실행되며, 만약 `reconcile`이 실제 쓰기(재인덱싱/고아 삭제)를 수행해야 할 때만 `command_lock`을 획득하여, 변경 사항이 없는 일반적인 기동 시에는 읽기 요청(`search_knowledge`, `read_knowledge`, `ping`)이 블로킹 없이 즉시 처리되도록 해야 한다.
  - **파일 메타데이터(`mtime_ns`, `file_size`) 및 `version_hash` 스키마 승격 (`core/src/index/mod.rs`, `core/src/vault/mod.rs`):**
    - `knowledge_index` 테이블에 `version_hash TEXT`, `file_mtime_ns INTEGER`, `file_size INTEGER` 컬럼(또는 동등한 인덱스 컬럼)을 명시적으로 유지하여, `list_index_hashes()`가 매번 `metadata` JSON 문자열 전체를 파싱하지 않고 SQL 컬럼에서 즉시 조회하도록 해야 한다.
    - `DocumentStore`는 디렉터리 엔트리 스캔 시 파일 본문을 열지 않고 `(id, file_mtime_ns, file_size)` 목록만 초고속으로 반환하는 경량 메타데이터 스캔 인터페이스(`list_document_Metadata` 또는 동등 메서드)를 제공해야 한다.
    - `reconcile` 수행 시 특정 문서의 `(file_mtime_ns, file_size)`가 인덱스에 기록된 값과 일치하고 `file_size > 0`인 경우, 해당 `.md` 파일의 디스크 본문 읽기(`read_to_string`) 및 `parse_markdown()`을 건너뛰어야 한다 (`MET-STATIC-04`). 메타데이터가 다르거나 인덱스에 없는 파일에 대해서만 본문을 읽고 `version_hash`를 대조하여 재인덱싱 여부를 결정한다.
  - **단일 배치 트랜잭션 재인덱싱 (`MET-STATIC-05`):**
    - `reconcile` 중 $K$개의 문서를 재인덱싱하고 $D$개의 고아 인덱스를 삭제할 때, 문서마다 개별 `BEGIN IMMEDIATE ... COMMIT`을 호출하지 않고 `IndexStore::batch_reconcile(upserts, delete_ids)` (또는 동등한 배치 메서드)를 통해 **단 1회의 SQLite 트랜잭션**으로 일괄 커밋해야 한다 (`MET-STATIC-05`: $K > 0$일 때 트랜잭션 횟수 $= 1$).

### 3.3 REQ-PERF-003: SQLite Reader 커넥션 풀링 및 SQL 필터 푸시다운 (Filter Pushdown)
- **3.3.1 Description and Priority:** 매우 높은 우선순위 (`DEF-OBS-05` 해결). 매 검색 요청마다 발생하던 `sqlite3_open()` 시스템 콜과 전체 후보 행 JSON 파싱 비용을 제거하고, 후처리 필터링으로 인한 검색 누락(False Zero-Hit) 결함을 근본적으로 수정합니다.
- **3.3.2 Stimulus/Response Sequences:**
  - *Stimulus 1:* 에이전트가 세션 중 `search_knowledge`를 60회 호출함.
  - *Response 1:* `SqliteFtsIndexStore`는 매번 `sqlite3_open()`을 호출하지 않고 내부 Reader 커넥션 풀(재사용 가능한 Read-only WAL 커넥션 객체)에서 이미 초기화된 `SqliteConnection`을 즉시 대여하여 쿼리를 실행함 (`MET-STATIC-06`).
  - *Stimulus 2:* 3,000개의 문서 중 `authority = 0.6`인 하위 문서 5개만 `"incident"` 태그를 가지고 있을 때, 에이전트가 `filters: { tags: ["incident"] }, limit: 5`로 검색을 요청함.
  - *Response 2:* SQL 쿼리의 `WHERE` 절에서 `source_class`, `source_type`, `min_authority`, `tags` 필터가 먼저 평가된 후 `ORDER BY ... LIMIT ?`가 적용되어, 상위 20개(`limit * 4`)에 들지 못해 탈락하는 일 없이 5건의 문서가 100% 정확하게 반환됨 (`MET-STATIC-07`).
- **3.3.3 Functional Requirements:**
  - **Reader 커넥션 재사용 풀 (`core/src/index/mod.rs`):**
    - `SqliteFtsIndexStore`는 읽기 전용 SQLite 커넥션(`PRAGMA query_only=ON; PRAGMA busy_timeout=5000;`이 기적용된 `SqliteConnection`)을 보관하는 스레드 안전 풀(예: `Mutex<Vec<SqliteConnection>>`, 최대 풀 크기 4~8)을 내장해야 한다.
    - `search_timed()` 및 `list_index_hashes()`는 풀에서 유휴 커넥션을 꺼내 사용하고 완료 후 풀에 반납(RAII 가드)해야 하며, 풀에 유휴 커넥션이 없을 때만 신규 커넥션을 생성해야 한다 (`MET-STATIC-06`: 순차 검색 $Q$회 수행 시 총 `sqlite3_open` 횟수 $= 1$).
  - **SQL `WHERE` 절 필터 푸시다운 (`MET-STATIC-07`):**
    - `source_class`, `source_type`, `min_authority` 필터 조건은 Rust 후처리 루프가 아닌 SQL 쿼리의 `WHERE` 절(`idx.source_class = ?`, `idx.source_type = ?`, `idx.authority >= ?`)에 직접 바인딩되어 `LIMIT` 절 이전에 평가되어야 한다.
    - `tags` 필터 또한 `LIMIT` 절로 잘리기 전에 SQL 계층에서 선행 필터링(예: 정규화된 소문자 태그 구분 문자열 컬럼 `tags_norm`에 대한 `INSTR(idx.tags_norm, '|tag|') > 0` 또는 SQLite JSON/서브쿼리 매칭)되어야 하며, 이를 통해 조건에 부합하는 문서가 `limit * 4` 컷오프 때문에 누락되는 현상(False Negative)이 절대 발생하지 않아야 한다.
    - 자주 사용되는 필터 및 정렬 조건을 위해 `knowledge_index(source_class, source_type, authority DESC, created_at DESC)` 복합 인덱스를 생성해야 한다.

### 3.4 REQ-PERF-004: 다중 키워드 2단계 자동 `OR` 폴백 검색 (Adaptive FTS5 Recall)
- **3.4.1 Description and Priority:** 높은 우선순위 (`DEF-OBS-06` 해결). AI 에이전트가 다중 키워드로 질의할 때 발생하는 Zero-Hit 및 반복 재질의 루프(Retry Churn)를 억제합니다.
- **3.4.2 Stimulus/Response Sequences:**
  - *Stimulus:* 에이전트가 명시적 불리언 연산자(`AND`/`OR`) 없이 3개 이상의 키워드가 포함된 질의(예: `"payment stripe timeout webhook"`)로 `search_knowledge`를 호출했으나, 4개 단어가 모두 들어있는 문서는 0건이고 3개 단어가 들어있는 문서는 2건 존재함.
  - *Response:* 검색 엔진은 1차 `AND` 쿼리(`"payment" AND "stripe" AND "timeout" AND "webhook"`) 결과가 `0`건임을 감지하면, 즉시 동일 커넥션 내에서 2차 `OR` 폴백 쿼리(`"payment" OR "stripe" OR "timeout" OR "webhook"`)를 실행하고 BM25 연관도(`bm25(knowledge_fts)`) 및 권위(`authority`) 순으로 정렬하여 가장 많은 키워드가 일치하는 상위 2건의 문서를 반환함 (`MET-STATIC-08`).
- **3.4.3 Functional Requirements:**
  - **2단계 Adaptive FTS5 Query (`AND` $\rightarrow$ `OR` BM25 Fallback):**
    - 사용자가 명시적으로 `AND` 또는 `OR` 연산자를 입력하지 않은 다중 토큰 질의(`query_token_count >= 2`)에 대해:
      1. **1단계 (Strict Precision):** 모든 토큰을 `AND`로 결합한 FTS5 쿼리를 실행한다.
      2. **2단계 (Recall Fallback):** 만약 1단계 매칭 결과가 `0`건(`results.is_empty()`)이면, 토큰들을 `OR`로 결합한 FTS5 쿼리(`build_fts_or_query`)를 자동으로 실행하고 `ORDER BY bm25(knowledge_fts) ASC, idx.authority DESC` 기준으로 가장 많은 검색어를 포함한 문서를 우선 반환해야 한다 (`MET-STATIC-08`).
    - 관측성 메트릭(`SearchMetrics`, `EPIC-OBS-01` `REQ-OBS-002`)에 `fallback_used: bool` 필드를 추가(또는 기록)하여 `omk stats`에서 자동 폴백으로 구제된 검색 횟수를 추적할 수 있어야 한다.

### 3.5 REQ-PERF-005: 쓰기 파이프라인(`upsert_knowledge`) I/O 및 직렬화 최적화
- **3.5.1 Description and Priority:** 중간~높은 우선순위 (`DEF-OBS-04`, `DEF-OBS-05` 해결). 지식 저장 시 발생하는 불필요한 메모리 할당 및 SQLite 문장 재컴파일 오버헤드를 줄이고, 테스트/배치 환경을 위한 동기화 제어 옵션을 제공합니다.
- **3.5.2 Stimulus/Response Sequences:**
  - *Stimulus:* 에이전트가 `upsert_knowledge`를 호출하여 신규 지식을 생성하거나 기존 지식을 갱신함.
  - *Response:* 파이프라인은 단일 패스로 마크다운과 인덱스 메타데이터를 직렬화하고, 원자적 파일 교체 및 SQLite 인덱스 갱신을 최소한의 락 점유 시간 내에 완료함.
- **3.5.3 Functional Requirements:**
  - `upsert_index` 수행 시 기존 인덱스에 동일 `id`와 동일 `version_hash`가 이미 등록되어 있다면(중복 Upsert), 불필요한 FTS5 `DELETE` + `INSERT` 재토큰화 트랜잭션을 생략(No-op short-circuit)할 수 있어야 한다.
  - `LocalMarkdownDocumentStore::put_document_timed`는 기본적으로 원자적 `.tmp` 쓰기 + `sync_all()`(`fsync`) + `rename`을 수행하여 데이터 무결성을 보장하되, 합성 벤치마크나 대량 테스트 시 디스크 플래터/SSD 하드웨어 지연과 순수 소프트웨어 파이프라인 지연을 분리 측정할 수 있도록 설정(`sync_policy`) 주입을 지원해야 한다.

### 3.6 REQ-PERF-006: 정적/동적 성능 검증 CLI (`omk bench`) 및 불변식 자동화 테스트
- **3.6.1 Description and Priority:** 매우 높은 우선순위 (필수 조건 달성 수단). 정의된 정적 복잡도 지표(`MET-STATIC-01` ~ `08`)와 가상 조건 워크로드 SLA(주 5일, 일 8시간, 시간당 10턴, 1년 누적 문서)를 누구든 단일 명령어와 테스트로 재현·검증할 수 있도록 합니다.
- **3.6.2 Stimulus/Response Sequences:**
  - *Stimulus 1:* 개발자 또는 CI 파이프라인이 `cargo test`를 실행함.
  - *Response 1:* 테스트 스위트는 시간(ms)에 의존하지 않고 내부 계측 카운터(`uds_connect_count`, `sqlite_open_count`, `markdown_parse_count`, `sqlite_tx_count`, `fallback_used` 등)를 검사하여 `MET-STATIC-01` ~ `MET-STATIC-08` 불변식이 100% 충족됨을 결정론적으로 검증함.
  - *Stimulus 2:* 사용자가 `omk bench [--docs <N>] [--turns <T>] [--json]`을 실행함 (기본값: `--docs 3000 --turns 80`, 즉 1년 차 누적 금고에서 하루 8시간 업무량 시뮬레이션).
  - *Response 2:* 격리된 임시 Vault에 $N$개의 합성 마크다운 문서를 시드(Seed)한 후, 증분 `reconcile` 검증과 $T$번의 사용자 턴(턴당 검색/읽기/쓰기 혼합 1.5회 = 총 $1.5 \times T$회 RPC)을 시뮬레이션하고, **정적 불변식 통과 여부(PASS/FAIL)**와 **도구별 P50/P95/P99 지연 시간 및 SLA 달성 여부**를 출력함.
- **3.6.3 Functional Requirements:**
  - **정적 카운터 노출 (`omk_core` 및 `omk`):**
    - `SqliteFtsIndexStore`는 누적 `sqlite3_open` 호출 횟수(`open_count()`), 누적 트랜잭션 커밋 횟수(`tx_commit_count()`)를 조회할 수 있는 원자적 카운터 메서드를 제공해야 한다.
    - `LocalMarkdownDocumentStore`는 누적 파일 본문 파싱 횟수(`parse_count()`) 및 파일 쓰기 횟수(`write_count()`)를 조회할 수 있는 원자적 카운터 메서드를 제공해야 한다.
    - `McpProxy` 및 `DaemonServer`는 누적 소켓 연결 횟수(`connect_count()` / `accepted_connections()`)를 조회할 수 있는 원자적 카운터 메서드를 제공해야 한다.
  - **`omk bench` 서브커맨드 (`skill/src/main.rs`):**
    - CLI는 `omk bench [--docs <N>] [--turns <T>] [--yes | -y] [--json]` 명령어를 지원해야 한다 (기본값: `--docs 3000`, `--turns 80`).
    - **디스크 리소스 보호 임계치 및 상시 사전 승인 인터랙션 (`REQ-PERF-006` Safety Guard):**
      - **절대 상한선 (Hard Cap Limit):** 로컬 디스크 및 Inode 고갈 사고를 원천 차단하기 위해 `--docs`는 최대 **`100,000`**, `--turns`는 최대 **`10,000`**을 초과할 수 없으며, 초과 입력 시 즉시 인자 검증 에러(`INVALID_ARGUMENT`)를 반환하고 실행을 차단해야 한다.
      - **권고 임계치 및 초과 경고 (Soft Warning Threshold):** `--docs > 10,000` 또는 `--turns > 1,000`일 경우, 실행 전 권고 임계치 초과 및 디스크 I/O·소요 시간 증가에 대한 명시적 경고 문구(`[WARN]`)를 출력해야 한다.
      - **상시 예상 디스크 사용량 고지 및 사용자 승인 프롬프트 (Always-on Pre-flight Confirmation):** 인터랙티브 터미널(TTY)에서 실행 시(단, `--yes`/`-y` 또는 `--json` 모드가 아닌 경우), 항상 시드 문서 수($N$), 시뮬레이션 턴 수($T$), **예상 임시 디스크 사용량(Estimated Temp Disk Usage, 문서당 약 `7.5 KB` + 인덱스 오버헤드 반영)**을 먼저 출력하고 사용자 승인(`Proceed with benchmark? [y/N]:`)을 받아야 한다. 사용자가 `y` 또는 `yes`를 입력하지 않으면 벤치마크를 즉시 취소하고 임시 파일을 생성하지 않아야 한다. 비대화형(Non-TTY) 환경에서 권고 임계치(`--docs > 10,000` 또는 `--turns > 1,000`)를 초과하여 실행하려면 반드시 `--yes`(`-y`) 플래그를 명시해야 한다.
      - **RAII 임시 디렉터리 자동 정리 (`Drop` Guard):** 벤치마크 실행 도중 에러 조기 반환(`?`)이나 패닉이 발생하더라도 OS 임시 디렉터리(`omk_bench_<pid>_<nanos>`)가 누수 없이 삭제되도록 RAII `Drop` 가드로 생명주기를 보호해야 한다.
    - `omk bench`는 사용자의 실제 Vault를 건드리지 않고 임시 디렉터리에 $N$개의 문서를 고속 시드한 뒤 다음 **2대 검증 리포트**를 출력해야 한다:
      1. **Static Invariant Checks (정적 결정론적 지표 검증):**
         - `MET-STATIC-01` (Persistent UDS Connection: `connect_count == 1` across $M$ calls) : `PASS / FAIL`
         - `MET-STATIC-03` & `04` (Incremental No-op Reconcile over $N$ docs: `body_parses == 0`) : `PASS / FAIL`
         - `MET-STATIC-05` (Batched Reconcile Transaction: `tx_commits == 1` for $N$ docs) : `PASS / FAIL`
         - `MET-STATIC-06` (SQLite Reader Connection Reuse: `sqlite_opens == 1` across $Q$ searches) : `PASS / FAIL`
         - `MET-STATIC-07` & `08` (Filter Pushdown & Multi-token `OR` Fallback Recall: `zero_hit_rate == 0%` on partial-match workload) : `PASS / FAIL`
      2. **Synthetic Workload Latency SLAs ($N$ docs, $T$ turns = $1.5T$ RPC calls):**
         - `search_knowledge`, `read_knowledge`, `upsert_knowledge`, `incremental_reconcile` 각각의 `Count`, `P50`, `P95`, `P99`, `Max` (`ms`) 및 하루 80턴 총 소요 시간(`total_workload_ms`)과 SLA 통과 여부를 출력해야 한다.

---

## 4. Quality Attributes (Non-Functional Requirements)

### 4.1 Determinism & Test Reproducibility (결정론적 재현성)
- **Zero Flaky Performance Tests:** `cargo test`에 포함되는 모든 성능 검증 테스트는 CPU 클럭이나 디스크 부하에 따라 간헐적으로 실패(Flaky)하는 `elapsed < X ms` 방식에만 의존해서는 안 되며, 반드시 `MET-STATIC-01` ~ `MET-STATIC-08`의 **정적 카운터 불변식(Exact Counter Assertions)**을 1차 합격 기준으로 삼아야 한다.

### 4.2 Scalability (장기 축적 확장성)
- **Sub-Linear / Constant Startup & Search Scaling:** 로컬 금고의 문서 수 $N$이 1개월 차($250$개)에서 1년 차($3,000 \sim 5,000$개)로 **20배** 증가하더라도:
  - 데몬 콜드 스타트 소켓 바인딩 지연은 문서 수 $N$과 무관한 **$O(1)$ 상수 시간**이어야 한다.
  - 무변경 상태에서 `reconcile` 시 본문 파싱 횟수는 $N$과 무관하게 **$0$회**여야 한다.
  - FTS5 역인덱스 및 SQL 인덱스 푸시다운을 통해 `search_knowledge`의 P95 지연 시간은 $N = 3,000$에서도 **$4.0\text{ ms}$ 이하**를 유지해야 한다.

### 4.3 Backward Compatibility & Zero External Dependencies (`CONSTITUTION.md` §1, §2)
- 모든 최적화(영속 UDS 연결, 블로킹 `accept` + Self-wake, SQLite 커넥션 풀, 증분 `reconcile`, 2단계 FTS5 폴백, `omk bench`)는 외부 서드파티 크레이트 추가 없이 Rust 표준 라이브러리(`std`)와 시스템 `libsqlite3` C ABI만으로 구현되어야 한다.
- 기존 MCP 프로토콜(`EPIC-IPC-01`) 및 관측성 로그 포맷(`EPIC-OBS-01`)과의 100% 하위 호환성을 유지해야 한다.

---

## 5. Traceability Matrix (추적성 매트릭스)

| Requirement ID | Requirement Title | Resolved Bottleneck (`EPIC-OBS-01`) | Static Metric ID | Target Module / File | Verification Method |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **`REQ-PERF-001`** | 영속 IPC 세션 연결 및 무지터(Zero-Jitter) 이벤트 수신 루프 | `DEF-OBS-01`<br>`DEF-OBS-03` | `MET-STATIC-01`<br>`MET-STATIC-02` | `skill/src/proxy/mod.rs`<br>`skill/src/daemon/mod.rs` | Integration Test ($M=50$ 연속 요청 시 `accepted_connections == 1` 및 15ms 폴링 슬립 제거 검증) |
| **`REQ-PERF-002`** | 콜드 스타트 Fast-Bind 및 메타데이터 기반 증분/배치 `reconcile` | `DEF-OBS-02` | `MET-STATIC-03`<br>`MET-STATIC-04`<br>`MET-STATIC-05` | `skill/src/daemon/mod.rs`<br>`core/src/search/mod.rs`<br>`core/src/vault/mod.rs`<br>`core/src/index/mod.rs` | Unit & Integration Test ($N=1,000$ 무변경 대사 시 `parse_count == 0`, 초기 일괄 인덱싱 시 `tx_commit_count == 1` 검증) |
| **`REQ-PERF-003`** | SQLite Reader 커넥션 풀링 및 SQL 필터 푸시다운 | `DEF-OBS-05` | `MET-STATIC-06`<br>`MET-STATIC-07` | `core/src/index/mod.rs` | Unit Test ($Q=100$ 검색 시 `open_count == 1`, 하위 권위 문서 태그 필터링 Recall 100% 검증) |
| **`REQ-PERF-004`** | 다중 키워드 2단계 자동 `OR` 폴백 검색 (Adaptive FTS5 Recall) | `DEF-OBS-06` | `MET-STATIC-08` | `core/src/index/mod.rs`<br>`core/src/search/mod.rs`<br>`core/src/domain/mod.rs` | Unit Test (다중 키워드 부분 일치 시 `OR` 폴백 적중 검증) |
| **`REQ-PERF-005`** | 쓰기 파이프라인(`upsert_knowledge`) I/O 및 직렬화 최적화 | `DEF-OBS-04`<br>`DEF-OBS-05` | `MET-STATIC-05` | `core/src/search/mod.rs`<br>`core/src/vault/mod.rs`<br>`core/src/index/mod.rs` | Unit Test (중복 해시 Upsert 시 불필요한 인덱스 재작성 생략 및 파이프라인 타이밍 검증) |
| **`REQ-PERF-006`** | 정적/동적 성능 검증 CLI (`omk bench`) 및 불변식 자동화 테스트 | 전 구간 (`DEF-OBS-01` ~ `06`) | `MET-STATIC-01` ~ `08` 전체 | `skill/src/main.rs`<br>`skill/src/daemon/mod.rs`<br>`core/src/index/mod.rs`<br>`core/src/vault/mod.rs` | CLI Integration Test (`omk bench --docs 300 --turns 10 --json` 실행 시 모든 Static Invariants `PASS` 검증) |

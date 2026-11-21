# omk (oh my knowledge) - Project Constitution

## 1. Architecture & Design Principles
- **Intentionality & Quality Attributes (일련의 의도와 품질 속성 중심 아키텍처):**
  - 아키텍처는 단순히 기능이 동작하도록 구조를 나누는 일이 아니라, 목표 품질 속성(성능, 보안·프라이버시, 데이터 무결성, 이식성)을 가장 효과적으로 달성하기 위해 최상위 설계 의도부터 핵심 하위 세부 사항까지 일관되게 연결하고 제약하는 **일련의 의도(Chain of Intentionality)**다.
  - 거시적 구조와 미시적 구현 세부 사항을 인위적인 경계로 나누지 않는다. 시스템 전반의 품질 속성 달성에 직결되는 세부 사항은 아키텍처적 제약으로 엄격히 통제하고, 그 외의 일반 세부 사항은 합리적인 구현 자율성에 열어둔다.
- **Agent-Agnostic (에이전트 범용성):** 특정 플랫폼이나 AI 클라이언트(IDE 등)에 종속되지 않는 범용성을 유지한다.
- **Environment Isolation (환경 독립성):** 사용자 로컬 환경의 제약이나 특정 런타임에 의존하지 않고 독립적으로 동작할 수 있어야 한다.
- **Core Isolation (코어 격리):** 핵심 비즈니스 로직은 통신 방식이나 외부 인프라의 변화로부터 안전하게 격리되어 보호되어야 한다.

## 2. Governance (거버넌스 및 예외 처리)
- **거버넌스 및 예외 처리 원칙:** 모든 구조적 변경 사항은 헌법과 지침을 기준으로 검토되어야 하며, 만약 원칙을 벗어나는 예외 상황이나 우회 방법(Workaround)이 필요할 경우, 타당성을 문서화하고 반드시 사용자(기술 리더)의 사전 승인을 거쳐야 한다.
- **Third-Party Dependencies (외부 의존성 관리):** 새로운 서드파티 라이브러리(Crate)를 도입해야 할 경우 임의로 의존성을 추가하지 마라. 의존성 추가가 가져올 트레이드오프(빌드 시간, 바이너리 크기, 보안 등)를 분석하여 사용자(기술 리더)에게 먼저 제안하고, 승인된 경우에만 추가해야 한다.
- **Human-in-the-Loop Gate (원격 상태 변경 및 병합 통제권):** Pull Request 제도는 `main` 브랜치 반영과 외부 배포에 앞서 인간 기술 리더가 변경 의도와 품질을 최종 검증하는 일련의 의도(Chain of Intentionality)의 최종 통제 관문이다. `main` 브랜치 병합(`gh pr merge`), GitHub 릴리스 생성·수정·삭제, 저장소 설정 및 Ruleset 변경, 쓰기 성격의 원격 API 호출은 인간 기술 리더의 고유 권한이며, AI 에이전트가 이를 임의로 대행하거나 자신이 올린 PR을 직접 병합(자가 병합, Self-Merge)하는 행위를 금지한다. 단계별 승인 경계와 기본 작업 상한선은 [`AGENTS.md`](AGENTS.md) §1을 따른다.

## 3. Requirements Management (요구사항 관리 체계)
### 3.1 Core Principles (기본 원칙)
- **Update First:** 코드를 작성하거나 수정하기 전, 기획의 단일 진실 공급원(SSOT)인 [`docs/requirements/index.md`](docs/requirements/index.md)를 우선 최신화하라. 구두 합의만으로 코드를 구현하지 않는다.
- **Problem-Driven:** 사용자의 표면적 지시(How)에 맹목적으로 따르지 마라. 본질적 목적(What/Why)을 파악하고, 빈틈이 있다면 구현 전 역제안하라.
- **Quality-Driven Intentionality (품질 속성 중심의 의도·제약 명세):** 요구사항을 정의하고 설계할 때 단순한 기능 동작 명세에 그치지 않고, **목표 품질 속성(Quality Attribute) → 아키텍처적 의도(High-level Intent) → 강제되는 핵심 세부 제약(Low-level Constraint / Invariant)**의 연결 고리를 명확히 정의하고 검증 기준까지 함께 명문화하라.
- **Traceable Coding:** 기획 문서에 명시되지 않은 기능을 임의로 확장(Over-engineering)하지 마라. 모든 코드는 명시된 요구사항을 달성하기 위한 목적만 가져야 한다.
- **Impact Analysis & Deduplication (간섭 및 중복 해결):** 요구사항이 축적되는 과정에서 새로운 요구사항이 기존 기능과 충돌하거나 중복(Overlapping)되는지 반드시 사전 검토하라. 간섭이나 중복이 발생할 경우 무분별하게 새 ID를 늘리지 말고, 기존 요구사항을 병합(Merge)하거나 갱신하여 문서의 파편화와 논리적 모순을 방지해야 한다. 해결 방향은 사용자와 합의 후 진행한다.

### 3.2 Management Framework (관리 프레임워크)
- 요구사항의 계층 구조, 식별자 네이밍 체계(`[Type]-[Domain]-[Seq]`) 및 세부 추적 방법론은 [`docs/requirements/index.md`](docs/requirements/index.md)의 관리 프레임워크를 따른다.

## 4. Documentation & Knowledge Management (문서 및 지식 관리)
- **Knowledge Map Lifecycle (지식 맵 관리 규칙):**
  1. **내용 갱신:** 코드 아키텍처나 정책이 변경될 경우, 반드시 지식 맵에 등재된 **기존 문서의 내용**을 먼저 갱신하여 단일 진실 공급원(SSOT)을 유지하라.
  2. **구조 갱신:** 기존 문서만으로 커버할 수 없는 새로운 성격의 규칙/가이드 문서를 신규 생성한 경우, 생성 직후 **반드시 [`AGENTS.md`](AGENTS.md)의 지식 맵 리스트에 해당 문서의 경로와 1줄 요약(역할)을 추가**하라.
  3. **Cross-Referencing (상호 참조 링킹):** 지식 맵 내에서 다른 문서를 참조할 때는 단순 텍스트 표기에 그치지 않고, 반드시 클릭 가능한 마크다운 상대 경로 링크 포맷(예: `[docs/architecture.md](docs/architecture.md)`)을 사용하여 문서 간의 탐색성을 극대화하라.

# omk (oh my knowledge) - System Instructions

## 1. AI Collaboration Rules (AI 협업 지침)
- **Think Before Code:** 코드를 작성·수정하기 전 단순히 기능 동작 여부만 보지 말고, [`CONSTITUTION.md`](CONSTITUTION.md) §1과 [`docs/architecture.md`](docs/architecture.md) §6에 정의된 **품질 속성을 향한 일련의 의도(Chain of Intentionality)**를 먼저 파악하여 단계별로 검토하고 사용자에게 짧게 브리핑하라. 품질 속성에 직결된 아키텍처적 세부 제약은 사전 승인([`CONSTITUTION.md`](CONSTITUTION.md) §2) 없이 절대 임의로 우회하거나 완화하지 말고, 그 외 일반 구현 세부 사항만 유연하게 다루라.
- **Discuss Trade-offs:** 트레이드 오프(성능, 복잡도, UX 등)가 발생하는 모든 결정 사항은 투명하게 드러내고, 거버넌스 원칙에 따라 반드시 사용자와 합의한 후 진행하라.
- **Atomic Commits:** 한 번에 너무 많은 책임을 가진 코드를 작성하지 마라. 테스트 가능한 최소 단위로 나누어 구현하라.
- **No Hallucination in APIs:** 표준 라이브러리나 명확히 명시된 패키지 외에 존재하지 않는 의존성을 가상으로 만들어내지 마라.
- **Language:** 코드 내의 변수명, 함수명, 시스템 로그, 주석, 표준 오픈소스 문서(예: `README.md`)는 **영어(English)**로 작성하고, 사용자와의 소통 및 내부 기획 문서, 프롬프트 응답은 **한국어(Korean)**를 기본으로 한다.
- **DRY Documentation:** 지침이나 문서를 작성할 때 다른 파일에 존재하는 규칙이나 맥락을 복사하거나 요약해 풀어쓰지 마라. 반드시 해당 파일의 링크나 경로만을 참조(Reference)하여 단일 진실 공급원(SSOT)을 지켜라.
- **Document SRP (문서 단일 책임 원칙):** 각 문서는 하나의 명확한 책임만 갖는다. 새로운 정보를 추가할 때는 내용의 성격을 파악하고 이에 부합하는 단 하나의 문서에만 기록하여 문서가 비대해지는 것을 막아라.
- **Pre-flight Check:** 새로운 규칙이나 문서 구조를 추가하기 전, 반드시 기존 지식 맵(Knowledge Map)의 파일들을 먼저 읽거나 검색하여 이미 동일한 성격의 룰이 존재하는지 파악하라.
- **Branch & PR Governance:** AI 에이전트(서브 에이전트 및 워커 포함)는 `main` 브랜치에 직접 커밋·푸시하지 말고, 반드시 [`CONTRIBUTING.md`](CONTRIBUTING.md)에 정의된 작업 브랜치 네이밍 규칙과 PR 작성 절차를 따르라. PR 병합과 릴리스 관리는 [`CONSTITUTION.md`](CONSTITUTION.md) §2 및 [`CONTRIBUTING.md`](CONTRIBUTING.md) §2에 따라 인간 기술 리더의 고유 권한이다.
- **Authority Boundary & Default Stop Line (4단계 승인 경계 및 기본 작업 상한선):** AI 에이전트는 사용자의 발화 의도와 명시적 승인 범위를 넘어 다음 단계로 임의 진행해서는 안 되며, 구현 지시 시 **로컬 커밋 완료 시점을 기본 작업 상한선(Default Stop Line)**으로 삼는다.
  1. **1단계 — 관찰·문제 제기·질문:** 현상 공유, 개선점 언급, 원인 질문, 설계 논의 단계에서는 읽기 전용 조사·원인 분석·트레이드오프 정리 및 선택지 제안까지만 수행하고 대기한다. (파일 수정, 브랜치 생성, 커밋, 원격 명령 일체 금지)
  2. **2단계 — 구현 지시 (기본 작업 상한선):** *"고쳐줘"*, *"구현해줘"*, *"문서 갱신해줘"* 등 명시적 구현 지시를 받으면 작업 브랜치 생성, SSOT 문서·코드 수정, 로컬 빌드·테스트·검증, **로컬 커밋(`git commit`)까지만 수행하고 즉시 중단**한다. (`git push`, `gh pr create`, `gh pr merge`, `gh release *` 등 모든 원격 상태 변경 금지)
  3. **3단계 — 원격 반영·PR 생성 지시:** 사용자가 *"푸시하고 PR 열어줘"* 등으로 명시적으로 요청한 경우에만 작업 브랜치 `git push`와 `gh pr create`를 수행하고, PR 링크와 검토 포인트를 보고한 뒤 중단한다. (`gh pr merge`, `gh release *`, `main` 직접 조작 금지)
  4. **4단계 — 병합·릴리스·저장소 설정 (인간 기술 리더 고유 권한):** PR 병합(`gh pr merge`), GitHub 릴리스 관리(`gh release create/edit/delete/upload/delete-asset`), 저장소 설정·Ruleset 변경(`gh repo edit/delete/archive/rename`), 쓰기용 `gh api` 및 원격 API 쓰기 호출은 어떤 경우에도 AI 에이전트가 실행할 수 없다.

## 2. Knowledge Map (지식 맵)
AI가 제품 개발을 성공적으로 진행하기 위해, 작업 전후로 아래 지식 맵을 참고하여 프로젝트 컨텍스트를 파악해야 한다.
지식 맵의 내용 및 구조 갱신에 대한 상세 조건은 [`CONSTITUTION.md`](CONSTITUTION.md)의 **"4. Documentation & Knowledge Management"** 원칙을 엄격히 따른다.
- **[`CONSTITUTION.md`](CONSTITUTION.md):** 프로젝트 거버넌스, 기본 설계 철학 및 요구사항 관리 체계 (프로젝트 헌법)
- **[`CONTRIBUTING.md`](CONTRIBUTING.md):** 오픈소스 기여 가이드, 작업 브랜치·커밋 규칙 및 PR(Squash Merge) 워크플로우
- **[`docs/coding_conventions.md`](docs/coding_conventions.md):** 코딩 컨벤션, 문서화 규칙 및 테스트 전략
- **[`docs/requirements/index.md`](docs/requirements/index.md):** 제품 요구사항 및 스펙 진입점(Entry Point)
- **[`docs/requirements/glossary.md`](docs/requirements/glossary.md):** 프로젝트 핵심 구성요소(omk CLI, omk Skill, AI Agent 등) 및 도메인 경계의 고유 명칭과 역할 정의 (용어 사전)
- **[`docs/architecture.md`](docs/architecture.md):** 시스템 세부 아키텍처 및 데이터 흐름 구조
- **[`DIRECTORY_LAYOUT.md`](DIRECTORY_LAYOUT.md):** 프로젝트 디렉터리 레이아웃 및 물리적 모듈 구조
- **[`docs/protocol_spec.json`](docs/protocol_spec.json):** JSON-RPC 기반 통신 프로토콜 규격

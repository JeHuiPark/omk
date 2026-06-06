# OMV (Oh My Vault) - System Instructions

## 1. AI Collaboration Rules (AI 협업 지침)
- **Think Before Code:** 코드를 수정하기 전, 변경 사항이 아키텍처 원칙에 부합하는지 단계별로 생각(Step-by-step thinking)하고 사용자에게 짧게 브리핑하라.
- **Discuss Trade-offs:** 트레이드 오프(성능, 복잡도, UX 등)가 발생하는 모든 결정 사항은 투명하게 드러내고, 거버넌스 원칙에 따라 반드시 사용자와 합의한 후 진행하라.
- **Atomic Commits:** 한 번에 너무 많은 책임을 가진 코드를 작성하지 마라. 테스트 가능한 최소 단위로 나누어 구현하라.
- **No Hallucination in APIs:** 표준 라이브러리나 명확히 명시된 패키지 외에 존재하지 않는 의존성을 가상으로 만들어내지 마라.
- **Language:** 코드 내의 변수명, 함수명, 시스템 로그, 주석, 표준 오픈소스 문서(예: `README.md`)는 **영어(English)**로 작성하고, 사용자와의 소통 및 내부 기획 문서, 프롬프트 응답은 **한국어(Korean)**를 기본으로 한다.
- **DRY Documentation:** 지침이나 문서를 작성할 때 다른 파일에 존재하는 규칙이나 맥락을 복사하거나 요약해 풀어쓰지 마라. 반드시 해당 파일의 링크나 경로만을 참조(Reference)하여 단일 진실 공급원(SSOT)을 지켜라.
- **Document SRP (문서 단일 책임 원칙):** 각 문서는 하나의 명확한 책임만 갖는다. 새로운 정보를 추가할 때는 내용의 성격을 파악하고 이에 부합하는 단 하나의 문서에만 기록하여 문서가 비대해지는 것을 막아라.
- **Pre-flight Check:** 새로운 규칙이나 문서 구조를 추가하기 전, 반드시 기존 지식 맵(Knowledge Map)의 파일들을 먼저 읽거나 검색하여 이미 동일한 성격의 룰이 존재하는지 파악하라.

## 2. Knowledge Map (지식 맵)
AI가 제품 개발을 성공적으로 진행하기 위해, 작업 전후로 아래 지식 맵을 참고하여 프로젝트 컨텍스트를 파악해야 한다.
지식 맵의 내용 및 구조 갱신에 대한 상세 조건은 [`CONSTITUTION.md`](CONSTITUTION.md)의 **"4. Documentation & Knowledge Management"** 원칙을 엄격히 따른다.
- **[`CONSTITUTION.md`](CONSTITUTION.md):** 프로젝트 거버넌스, 기본 설계 철학 및 요구사항 관리 체계 (프로젝트 헌법)
- **[`CONTRIBUTING.md`](CONTRIBUTING.md):** 오픈소스 기여 가이드 및 커밋 메시지 규칙
- **[`docs/coding_conventions.md`](docs/coding_conventions.md):** 코딩 컨벤션, 문서화 규칙 및 테스트 전략
- **[`docs/requirements/index.md`](docs/requirements/index.md):** 제품 요구사항 및 스펙 진입점(Entry Point)
- **[`docs/architecture.md`](docs/architecture.md):** 시스템 세부 아키텍처 및 데이터 흐름 구조
- **[`DIRECTORY_LAYOUT.md`](DIRECTORY_LAYOUT.md):** 프로젝트 디렉터리 레이아웃 및 물리적 모듈 구조
- **[`docs/protocol_spec.json`](docs/protocol_spec.json):** JSON-RPC 기반 통신 프로토콜 규격

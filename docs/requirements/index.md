# omk (oh my knowledge) - Requirements Index

이 문서는 omk 프로젝트의 전체 요구사항(Requirements)을 탐색하기 위한 진입점(Entry Point)입니다.
상세 기획 및 스펙을 확인할 때는 아래 링크된 문서들을 참조하십시오.

> [!IMPORTANT]
> **용어 및 도메인 경계:** 기획 및 개발 시 의사소통 혼선을 방지하기 위해 반드시 [용어 사전 (Glossary)](glossary.md)에 정의된 시스템 경계 및 고유 명칭(omk CLI, omk Skill, AI Agent 등)을 숙지하고 일관되게 사용하십시오.

## 1. Management Framework (요구사항 관리 체계)
- **Hierarchical Structure (3단계 계층 구조):** 
  1. **Business Requirements:** 제품이 해결하려는 거시적 목적과 프로젝트의 범위(Vision and Scope).
  2. **User Requirements:** 사용자가 시스템과 상호작용하며 달성하려는 구체적 시나리오(Use Case).
  3. **Functional Requirements:** 즉시 코드로 옮길 수 있는 시스템의 구체적 동작 명세(SRS).
- **Identification & Traceability (식별 및 추적 체계):** 
  - 모든 요구사항은 `[Type]-[Domain]-[Seq]` 형태의 고유 식별자(ID) 체계를 고정하여 사용한다.
    - `Type`: `EPIC` (에픽), `UC` (유스케이스), `REQ` (기능 요구사항)
    - `Domain`: 모듈 및 도메인 영역 (예: `CORE`, `SEARCH`, `IPC` 등 영문 대문자)
    - `Seq`: 순차 번호 (예: `01`, `001`)
    - *예시: `EPIC-SEARCH-01`, `UC-SEARCH-01`, `REQ-SEARCH-001`*
  - 이 식별자는 커밋 메시지, 소스 코드 주석, 테스트 코드 명세 등에 의무적으로 기재하여, 요구사항 문서와 구현 코드 간의 완벽한 1:1 양방향 추적성을 보장한다.

## 2. Business Requirements
- [Vision and Scope](vision_and_scope.md) : 프로젝트 정체성, 목적, 핵심 목표

## 3. User & Functional Requirements (Features)

- [EPIC-VAULT-01: Markdown 기반 영구 지식 저장소](features/EPIC-VAULT-01.md)
- [EPIC-DATA-01: 지식 데이터 모델 및 출처 관리](features/EPIC-DATA-01.md)
- [EPIC-SEARCH-01: 지식 검색 및 인덱싱](features/EPIC-SEARCH-01.md)
- [EPIC-IPC-01: MCP 통신 프로토콜 및 생명주기 (Vault Server & Session Proxy)](features/EPIC-IPC-01.md)
- [EPIC-CLI-01: 인간 친화적 CLI 사용성 및 다중 에이전트 하네스 프로비저닝](features/EPIC-CLI-01.md)
- [EPIC-OBS-01: 프라이버시 보호 기반 실행 관측성 및 성능 진단 체계](features/EPIC-OBS-01.md)
- [EPIC-PERF-01: 가상 워크로드 기반 성능 고도화 및 정적/결정론적 검증 지표 명세](features/EPIC-PERF-01.md)

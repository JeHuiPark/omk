# OMV (Oh My Vault) - Requirements Index

이 문서는 OMV 프로젝트의 전체 요구사항(Requirements)을 탐색하기 위한 진입점(Entry Point)입니다.
상세 기획 및 스펙을 확인할 때는 아래 링크된 문서들을 참조하십시오.

> [!IMPORTANT]
> **용어 및 도메인 경계:** 기획 및 개발 시 의사소통 혼선을 방지하기 위해 반드시 [용어 사전 (Glossary)](glossary.md)에 정의된 시스템 경계 및 고유 명칭(OMV CLI, OMV Skill, AI Agent 등)을 숙지하고 일관되게 사용하십시오.

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
*(현재 등록된 세부 기능 스펙이 없습니다. 기능 추가 시 `features/` 디렉터리에 파일을 생성하고 이곳에 링크하십시오.)*

- `features/` (TBD)

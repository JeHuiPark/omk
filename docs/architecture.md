# OMV (Oh My Vault) - Architecture & System Design

[`CONSTITUTION.md`](../CONSTITUTION.md)의 설계 철학을 구현하기 위한 시스템 아키텍처 명세.

## 1. 기술 스택 (Technology Stack)
- **Core Language:** Rust (메모리 안전성, GC 없는 고성능, 정적 단일 바이너리 빌드 지원)
- **Communication Data Format:** JSON (JSON-RPC 2.0 규격 사용)
- **Testing Framework:** 표준 빌트인 테스트 (`#[cfg(test)]`) 및 `mockall` (의존성 주입 및 모킹)

## 2. 통신 아키텍처 (Communication Architecture)
- **방식:** CLI 서브프로세스 기반 표준 입출력(Stdio) IPC
- **프로토콜:** JSON-RPC
- **제약사항:** 네트워크 소켓 및 백그라운드 데몬 구동 불가

## 3. 배포 및 실행 환경 (Deployment & Execution)
- **배포 형태:** 완전 독립형 단일 정적 바이너리 (Statically Linked Standalone Binary)
- **런타임 의존성:** 없음 (Python, Node.js, OS별 동적 라이브러리 사전 설치 불필요)
- **지원 타겟 (Target OS/Arch):**
  - macOS (Apple Silicon `aarch64` / Intel `x86_64`)
  - Windows (x64)

## 4. 소프트웨어 아키텍처 (Software Architecture)
- **구조:** 헥사고날 아키텍처 (Hexagonal Architecture)
- **의존성 규칙:** `core` 모듈은 통신(JSON-RPC) 및 외부 인프라(파일 I/O, DB 등) 구현체 참조 절대 불가
- **인터페이스 구현:** Rust 트레이트(Trait) 기반 Port-Adapter 패턴 및 의존성 역전(DIP) 적용
- **안정성 및 예외 처리:** 프로세스 생존 보장, 스트림 데드락 방지, JSON 포맷 에러 로그 반환 등 방어적 프로그래밍과 에러 처리에 대한 세부 구현 규칙은 [`docs/coding_conventions.md`](coding_conventions.md)를 참조하십시오.

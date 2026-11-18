# 보안 정책

런처는 플레이어의 Microsoft refresh token 을 OS 자격 증명 저장소에 두고, 받은 파일을 해시로 검증한 뒤 실행합니다.
이 경계를 넘는 문제(토큰 유출, 검증을 건너뛰는 다운로드, 업데이트 서명 우회, `hyperfarm://` 링크로 일으킬 수 있는
동작)를 찾으면 공개 이슈 대신 [비공개 보고](https://github.com/vjh0107/hyperfarm-launcher/security/advisories/new)로
알려 주세요.

지원하는 판은 가장 최근 릴리스 하나입니다. 설치된 런처는 켜질 때 스스로 최신 판으로 바뀝니다.

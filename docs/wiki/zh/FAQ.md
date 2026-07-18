# 常见问题

**English：** [en/FAQ.md](../en/FAQ.md)

### 现在能直接替换 Java `java-contract-match` 吗？

生产 MQ 路径尚未接通。进程壳已支持恢复 / 健康检查 / 指标与 **memory** 传输；RocketMQ 见 [rmq-spike](../../rmq-spike.md)。等价性通过黄金回放与影子方案验证。

### 为什么有两套引擎（`match-core` / `match-core-hp`）？

**等价轨**保留 Java 可观测行为（含已知 quirk）。**性能轨**用定点语义冲延迟，**不能**静默成为生产默认。

### 为什么我的「更快」数字被拒绝？

`fair_compare` 与性能策略要求 **成交率 > 0**。零成交虚高（部分 ART+SIMD demo 曾出现）标为 INVALID，不参与排名。

### 可以默认打开 ART 吗？

不可以。ART 在 `--features art` 后，且必须与 BTree 索引通过一致性测试。

### 如何报告安全问题？

见 [SECURITY.md](../../../SECURITY.md)。不要在公开 Issue 中披露未修复漏洞。

### 英文文档在哪？

- [README.md](../../../README.md)
- [Wiki English](../en/Home.md)
- [CONTRIBUTING.md](../../../CONTRIBUTING.md)

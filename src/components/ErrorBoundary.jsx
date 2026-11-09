import { Component } from "react";

/** 渲染出错时显示堆栈而不是白屏，便于定位前端崩溃 */
export default class ErrorBoundary extends Component {
  state = { error: null, info: null };

  static getDerivedStateFromError(error) {
    return { error };
  }

  componentDidCatch(error, info) {
    this.setState({ info });
    console.error("渲染崩溃:", error, info?.componentStack);
  }

  render() {
    const { error, info } = this.state;
    if (!error) return this.props.children;
    return (
      <div style={{ padding: 24, fontFamily: "ui-monospace, monospace", fontSize: 12, lineHeight: 1.6 }}>
        <div style={{ fontSize: 15, fontWeight: 700, marginBottom: 10, color: "#B42318" }}>
          页面渲染出错
        </div>
        <div style={{ whiteSpace: "pre-wrap", color: "#B42318", marginBottom: 12 }}>
          {String(error?.stack || error)}
        </div>
        {info?.componentStack ? (
          <div style={{ whiteSpace: "pre-wrap", color: "#5B6170" }}>{info.componentStack}</div>
        ) : null}
        <button
          style={{ marginTop: 16, padding: "6px 14px", border: "1px solid #ccc", borderRadius: 6 }}
          onClick={() => this.setState({ error: null, info: null })}
        >
          重试
        </button>
      </div>
    );
  }
}

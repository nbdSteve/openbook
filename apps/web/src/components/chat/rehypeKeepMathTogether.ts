import type { Element, ElementContent, Root, Text } from 'hast';

const OPENING = /([([{“‘"']+)$/;
const CLOSING = /^([)\]}”’"'.,;:!?]+)/;

function isInlineMath(node: ElementContent | undefined): node is Element {
  if (!node || node.type !== 'element' || node.tagName !== 'span') return false;
  const classes = node.properties?.className;
  return Array.isArray(classes) && classes.includes('katex') && !classes.includes('katex-display');
}

const isText = (node: ElementContent | undefined): node is Text => node?.type === 'text';

/** Wrap one inline formula and the punctuation that touches it, so a line break cannot split them. */
function keepTogether(parent: Element | Root): void {
  const children = parent.children as ElementContent[];
  for (let i = 0; i < children.length; i++) {
    const node = children[i];
    if (node.type === 'element' && !isInlineMath(node)) keepTogether(node);
    if (!isInlineMath(node)) continue;

    const before = children[i - 1];
    const after = children[i + 1];
    const head = isText(before) ? OPENING.exec(before.value)?.[1] : undefined;
    const tail = isText(after) ? CLOSING.exec(after.value)?.[1] : undefined;
    if (!head && !tail) continue;

    const wrapped: ElementContent[] = [];
    if (head) wrapped.push({ type: 'text', value: head });
    wrapped.push(node);
    if (tail) wrapped.push({ type: 'text', value: tail });
    const wrapper: Element = {
      type: 'element',
      tagName: 'span',
      properties: { style: 'white-space: nowrap' },
      children: wrapped,
    };

    if (tail && isText(after)) after.value = after.value.slice(tail.length);
    if (head && isText(before)) before.value = before.value.slice(0, before.value.length - head.length);
    children.splice(i, 1, wrapper);
  }
}

/** A rehype plugin: keep an inline formula together with directly adjacent opening and closing punctuation. */
export function rehypeKeepMathTogether() {
  return (tree: Root) => keepTogether(tree);
}

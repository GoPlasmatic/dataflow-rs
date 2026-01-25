import { getFunctionDisplayInfo } from '../../../types';

interface FunctionTypeBadgeProps {
  functionName: string;
  className?: string;
}

export function FunctionTypeBadge({ functionName, className = '' }: FunctionTypeBadgeProps) {
  const { label, colorClass, Icon } = getFunctionDisplayInfo(functionName);

  return (
    <span className={`df-function-badge ${colorClass} ${className}`}>
      <Icon size={12} />
      <span>{label}</span>
    </span>
  );
}

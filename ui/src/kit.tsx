import { Checkbox, CheckboxIndicator } from '@ui-sdk/react/checkbox';
import {
  SegmentedControl,
  SegmentedControlIndicator,
  SegmentedControlItem,
} from '@ui-sdk/react/segmented-control';
import { useId } from 'react';

export interface Option {
  value: string;
  label: string;
}

export function Pick(props: {
  label: string;
  value: string | null;
  items: Option[];
  onChange: (value: string) => void;
  className?: string;
}) {
  const id = useId();
  return (
    <div className={'wl-pick ' + (props.className || '')}>
      <span id={id} className="wl-pick-label">
        {props.label}
      </span>
      <SegmentedControl
        className="wl-seg"
        aria-labelledby={id}
        value={props.value}
        onValueChange={(next) => {
          if (typeof next === 'string') props.onChange(next);
        }}
      >
        <SegmentedControlIndicator className="wl-seg-indicator" />
        {props.items.map((item) => (
          <SegmentedControlItem key={item.value} value={item.value}>
            {item.label}
          </SegmentedControlItem>
        ))}
      </SegmentedControl>
    </div>
  );
}

export function Check(props: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <label className="wl-check">
      <Checkbox
        checked={props.checked}
        disabled={props.disabled}
        onCheckedChange={(checked) => props.onChange(checked === true)}
      >
        <CheckboxIndicator />
      </Checkbox>
      <span>{props.label}</span>
    </label>
  );
}

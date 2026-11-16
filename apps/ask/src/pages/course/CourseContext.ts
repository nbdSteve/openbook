import { useOutletContext } from 'react-router';
import type { ChatController } from '@/components/chat/useChatController';
import type { Course } from '@/services/api/types';

export interface CourseContextValue {
  course: Course;
  setCourse: (course: Course) => void;
  chat: ChatController;
}

export function useCourse(): CourseContextValue {
  return useOutletContext<CourseContextValue>();
}

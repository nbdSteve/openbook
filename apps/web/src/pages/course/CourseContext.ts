import { useOutletContext } from 'react-router';
import type { ChatController } from '@/components/chat/useChatController';
import type { Course } from '@/services/api/types';

/** What the pages of one course share. Both areas (Ask and Portal) provide it. */
export interface CourseContextValue {
  course: Course;
  setCourse: (course: Course) => void;
}

/** The Ask area adds the chat controller, so a streaming answer survives route changes inside the course. */
export interface ChatCourseContextValue extends CourseContextValue {
  chat: ChatController;
}

export function useCourse(): CourseContextValue {
  return useOutletContext<CourseContextValue>();
}

export function useChatCourse(): ChatCourseContextValue {
  return useOutletContext<ChatCourseContextValue>();
}
